This is currently mostly ChatGPT and not checked, I will improve this when I can.
# gpio2midi

**gpio2midi** is a Rust application that converts Raspberry Pi GPIO inputs—buttons and rotary encoders—into MIDI Control Change (CC) messages sent through a virtual MIDI port. It enables custom hardware MIDI controllers by bridging physical GPIO events to MIDI.

---

## Features

- Configurable controls via TOML file (buttons and rotary encoders).
- Debounced GPIO input handling.
- Supports absolute and relative rotary encoder modes.
- Sends MIDI CC messages through a virtual MIDI output port.

---

## Configuration

Controls are defined in a TOML configuration file with the following structures and default values:

### Button

- `pin`: GPIO pin number.
- `cc`: MIDI Control Change number to send.
- `pull_up` (optional, default: `false`): Enable internal pull-up resistor.
- `debounce_ms` (optional, default: `5` ms): Debounce duration in milliseconds.

### RotaryEncoder

- `pin_a`: GPIO pin for encoder channel A.
- `pin_b`: GPIO pin for encoder channel B.
- `cc`: MIDI Control Change number.
- `relative_value` (optional, default: `false`): Send relative increments/decrements if true.
- `debounce_ms` (optional, default: `1` ms): Debounce duration in milliseconds.

---

## Runtime Behavior

- Monitors GPIO interrupts for buttons and rotary encoders.
- Sends corresponding MIDI CC messages on state changes.
- Runs until interrupted.

---
