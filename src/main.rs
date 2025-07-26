use anyhow::Result;
use clap::Parser;
use ctrlc;
use homedir::get_my_home;
use midir::{MidiOutput, MidiOutputConnection};
use rppal::gpio::{Event, Gpio, InputPin, Level, Trigger};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// MIDI virtual port name
    #[arg(short, long, default_value = "gpio2midi")]
    port: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ControlConfig {
    Button {
        pin: u8,
        cc: u8,
        #[serde(default)]
        pull_up: bool,
        #[serde(default)]
        debounce_ms: Option<u64>,
    },
    RotaryEncoder {
        pin_a: u8,
        pin_b: u8,
        cc: u8,
        #[serde(default)]
        debounce_ms: Option<u64>,
        #[serde(default)]
        relative_value: bool,
    },
}

#[derive(Debug, Deserialize)]
struct Config {
    controls: Vec<ControlConfig>,
}

#[derive(Debug)]
enum ControlType {
    Button {
        cc: u8,
        pin: Arc<InputPin>,
    },
    RotaryEncoder {
        cc: u8,
        pin_a: Arc<InputPin>,
        pin_b: Arc<InputPin>,
        state: Arc<Mutex<RotaryEncoderState>>,
        relative: bool,
    },
}

fn send_cc(conn: &MidiOutputConnection, cc: u8, value: u8) {
    let _ = conn.send(&[0xB0, cc, value]);
}

// Gray code state machine transition table for rotary encoders
const TRANSITION_TABLE: [i8; 16] = [
     0, -1,  1,  0,
     1,  0,  0, -1,
    -1,  0,  0,  1,
     0,  1, -1,  0,
];

#[derive(Debug)]
struct RotaryEncoderState {
    prev_state: u8,
    accum: i8,
    value: u8,
}

impl RotaryEncoderState {
    fn new(a: Level, b: Level, initial_value: u8) -> Self {
        let prev_state = ((a == Level::High) as u8) << 1 | ((b == Level::High) as u8);
        Self {
            prev_state,
            accum: 0,
            value: initial_value,
        }
    }

    fn update(&mut self, a: Level, b: Level) -> Option<i8> {
        let new_state = ((a == Level::High) as u8) << 1 | ((b == Level::High) as u8);
        let index = (self.prev_state << 2) | new_state;
        let movement = TRANSITION_TABLE[index as usize];
        self.accum += movement;
        self.prev_state = new_state;

        if self.accum.abs() >= 4 {
            let step = self.accum.signum();
            self.accum = 0;
            Some(step)
        } else {
            None
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    let default_config = get_my_home()
        .map(|p| p.join("gpio2midi.toml"))
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?;

    let config_path = args.config.unwrap_or(default_config);
    let config: Config = toml::from_str(&fs::read_to_string(config_path)?)?;

    let gpio = Gpio::new()?;
    let midi_out = MidiOutput::new(&args.port)?;
    let conn = Arc::new(midi_out.create_virtual(&args.port)?);

    let mut pin_map: HashMap<u8, ControlType> = HashMap::new();

    for control in config.controls.iter() {
        match control {
            ControlConfig::Button { pin, cc, pull_up, debounce_ms } => {
                let mut gpio_pin = gpio.get(*pin)?;
                if *pull_up {
                    gpio_pin = gpio_pin.into_input_pullup();
                } else {
                    gpio_pin = gpio_pin.into_input_pulldown();
                }
                let debounce = debounce_ms.map(Duration::from_millis).or(Some(Duration::from_millis(5)));
                gpio_pin.set_interrupt(Trigger::Both, debounce)?;
                let arc_pin = Arc::new(gpio_pin);
                pin_map.insert(arc_pin.pin(), ControlType::Button { cc: *cc, pin: arc_pin });
            }
            ControlConfig::RotaryEncoder { pin_a, pin_b, cc, debounce_ms, relative_value } => {
                let mut a = gpio.get(*pin_a)?.into_input_pullup();
                let mut b = gpio.get(*pin_b)?.into_input_pullup();
                let debounce = debounce_ms.map(Duration::from_millis).or(Some(Duration::from_millis(1)));
                a.set_interrupt(Trigger::Both, debounce)?;
                let arc_a = Arc::new(a);
                let arc_b = Arc::new(b);
                let state = Arc::new(Mutex::new(RotaryEncoderState::new(arc_a.read(), arc_b.read(), 64)));
                pin_map.insert(
                    arc_a.pin(),
                    ControlType::RotaryEncoder {
                        cc: *cc,
                        pin_a: arc_a,
                        pin_b: arc_b,
                        state,
                        relative: *relative_value,
                    },
                );
            }
        }
    }

    let pin_refs: Vec<&InputPin> = pin_map.values().map(|control| match control {
        ControlType::Button { pin, .. } => pin.as_ref(),
        ControlType::RotaryEncoder { pin_a, .. } => pin_a.as_ref(),
    }).collect();

    // Handle Ctrl+C to exit cleanly
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })?;

    while running.load(Ordering::SeqCst) {
        if let Some((pin, _event)) = rppal::gpio::poll_interrupts(&pin_refs, true, Some(Duration::from_millis(100)))? {
            let pin_num = pin.pin();
            if let Some(control) = pin_map.get_mut(&pin_num) {
                match control {
                    ControlType::Button { cc, pin } => {
                        let value = if pin.read() == Level::Low { 127 } else { 0 };
                        send_cc(&conn, *cc, value);
                    }
                    ControlType::RotaryEncoder { cc, pin_a, pin_b, state, relative } => {
                        let mut s = state.lock().unwrap();
                        if let Some(dir) = s.update(pin_a.read(), pin_b.read()) {
                            if *relative {
                                let delta = if dir > 0 { 1 } else { 127 }; // 127 == -1
                                send_cc(&conn, *cc, delta);
                            } else {
                                if dir > 0 {
                                    s.value = s.value.saturating_add(1);
                                } else {
                                    s.value = s.value.saturating_sub(1);
                                }
                                send_cc(&conn, *cc, s.value);
                            }
                        }
                    }
                }
            }
        }
    }

    println!("Exiting cleanly.");
    Ok(())
}
