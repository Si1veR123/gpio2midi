use anyhow::Result;
use clap::Parser;
use homedir::my_home;
use midir::os::unix::VirtualOutput;
use midir::{MidiOutput, MidiOutputConnection};
use rppal::gpio::{Event, Gpio, InputPin, Level, Trigger};
use serde::Deserialize;
use tokio::time::sleep;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// MIDI virtual port name
    #[arg(short, long, default_value = "gpio2midi")]
    port: String,

    /// Polling rate for rotary encoder pins in hz
    #[arg(short, long, default_value_t = 4000.0)]
    polling_rate: f64
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ControlConfig {
    Button {
        pin: u8,
        cc: u8,
        #[serde(default)]
        pull_down: bool,
        #[serde(default)]
        debounce_ms: Option<u64>,
    },
    RotaryEncoder {
        pin_a: u8,
        pin_b: u8,
        cc: u8,
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
        // Keep alive for interrupt
        _pin: Arc<InputPin>
    },
    RotaryEncoder {
        cc: u8,
        pin_a: Arc<InputPin>,
        pin_b: Arc<InputPin>,
        state: Arc<Mutex<RotaryEncoderState>>,
        relative: bool,
    },
}

fn send_cc(conn: &mut MidiOutputConnection, cc: u8, value: u8) {
    if cfg!(feature = "print") {
        println!("Sending cc: {cc}, value: {value}");
    }

    let _ = conn.send(&[0xB0, cc, value]);
}

// Gray code state machine transition table for rotary encoders
const TRANSITION_TABLE: [i8; 16] = [
    // prev: 00
     0,  1, -1,  0,
    // prev: 01
    -1,  0,  0,  1,
    // prev: 10
     1,  0,  0, -1,
    // prev: 11
     0, -1,  1,  0,
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

        if new_state == self.prev_state {
            return None; // No change, ignore
        }

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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let default_config = my_home()?
        .ok_or_else(|| anyhow::anyhow!("Could not find home directory"))?
        .join("gpio2midi.toml");

    let config_path = args.config.unwrap_or(default_config);
    let config: Config = toml::from_str(&fs::read_to_string(config_path)?)?;

    let gpio = Gpio::new()?;
    let midi_out = MidiOutput::new(&args.port)?;
    let conn = Arc::new(Mutex::new(midi_out.create_virtual(&args.port).map_err(|e| anyhow::anyhow!("{e}"))?));

    let (tx, mut rx) = mpsc::channel::<(u8, Event)>(100);

    let mut pin_map: HashMap<u8, ControlType> = HashMap::new();

    for control in config.controls.iter() {
        match control {
            ControlConfig::Button { pin, cc, pull_down, debounce_ms } => {
                let pin = *pin;
                let gpio_pin = gpio.get(pin)?;
                let mut gpio_in_pin: InputPin;
                if *pull_down {
                    gpio_in_pin = gpio_pin.into_input_pulldown();
                } else {
                    gpio_in_pin = gpio_pin.into_input_pullup();
                }
                gpio_in_pin.set_reset_on_drop(false);
                let debounce = debounce_ms.map(Duration::from_millis).or(Some(Duration::from_millis(5)));
                let tx_clone = tx.clone();
                gpio_in_pin.set_async_interrupt(Trigger::Both, debounce, move |event| {
                    let _ = tx_clone.clone().try_send((pin, event));
                })?;
                pin_map.insert(pin, ControlType::Button { cc: *cc, _pin: Arc::new(gpio_in_pin) });
            }
            ControlConfig::RotaryEncoder { pin_a, pin_b, cc, relative_value } => {
                let (pin_a, pin_b) = (*pin_a, *pin_b);
                let a = gpio.get(pin_a)?.into_input_pullup();
                let b = gpio.get(pin_b)?.into_input_pullup();

                let arc_a = Arc::new(a);
                let arc_b = Arc::new(b);
                let state = Arc::new(Mutex::new(RotaryEncoderState::new(arc_a.read(), arc_b.read(), 64)));
                pin_map.insert(
                    arc_a.pin(),
                    ControlType::RotaryEncoder {
                        cc: *cc,
                        pin_a: arc_a.clone(),
                        pin_b: arc_b.clone(),
                        state: state.clone(),
                        relative: *relative_value,
                    },
                );
            }
        }
    }

    if cfg!(feature = "print") {
        println!("Using pins: {:?}", pin_map);
    }

    let cloned_conn = conn.clone();
    let pin_map = Arc::new(pin_map);
    let pin_map_clone = pin_map.clone();
    let mut previous_rotary_enc_levels = HashMap::new();
    let polling_sleep = Duration::from_secs_f64(1.0 / args.polling_rate as f64);
    tokio::spawn(async move {
        loop {
            for control in pin_map_clone.values() {
                if let ControlType::RotaryEncoder { cc, pin_a, pin_b, state, relative } = control {

                    let previous_levels_entry = previous_rotary_enc_levels.entry(pin_a.pin()).or_insert((Level::High, Level::High));

                    let a_val = pin_a.read();
                    let b_val = pin_b.read();

                    if (a_val, b_val) == *previous_levels_entry {
                        continue;
                    }

                    previous_levels_entry.0 = a_val;
                    previous_levels_entry.1 = b_val;

                    let mut s = state.lock().unwrap();
                    if let Some(dir) = s.update(a_val, b_val) {
                        if *relative {
                            let delta = if dir > 0 { 1 } else { 127 };
                            send_cc(&mut cloned_conn.lock().expect("Failed to lock midi port"), *cc, delta);
                        } else {
                            if dir > 0 {
                                s.value = s.value.saturating_add(1);
                            } else {
                                s.value = s.value.saturating_sub(1);
                            }
                            send_cc(&mut cloned_conn.lock().expect("Failed to lock midi port"), *cc, s.value);
                        }
                    }
                }
            }
            sleep(polling_sleep).await;
        }
    });

    while let Some((pin, event)) = rx.recv().await {
        if cfg!(feature = "print") {
            println!("Event on pin {pin}, {event:?}");
        }

        if let ControlType::Button { _pin, cc } = pin_map.get(&pin).expect("Pin should exist") {
            send_cc(&mut conn.lock().expect("Failed to lock midi port"), *cc, if event.trigger == Trigger::RisingEdge { 127 } else { 0 });
        }
        
    }

    println!("Exiting cleanly.");
    Ok(())
}
