# gpio2midi

**gpio2midi** is a Rust application that converts Raspberry Pi GPIO inputs - buttons and rotary encoders - into MIDI Control Change (CC) messages sent through a virtual MIDI port.


## Features

- Configurable controls via TOML file (buttons and rotary encoders).
- Debounced GPIO input handling.
- Supports absolute and relative rotary encoder modes.
- Sends MIDI CC messages through a virtual MIDI output port.


## Arguments
- `-c`/`--config` (optional, default: `~/gpio2midi.toml`): path to config file
- `-p`/`--port` (optional, default: `gpio2midi`): name of the virtual midi port 


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
- `relative_value` (optional, default: `false`): Send relative increments/decrements if true. `1`: increment. `127`: decrement.
- `debounce_ms` (optional, default: `1` ms): Debounce duration in milliseconds.

### Example
```toml
[[controls]]
type = "Button"
pin = 17
cc = 20
pull_up = true
debounce_ms = 50

[[controls]]
type = "Button"
pin = 27
cc = 21
# pull_up defaults to false
# debounce_ms is optional

[[controls]]
type = "RotaryEncoder"
pin_a = 5
pin_b = 6
cc = 22
debounce_ms = 5
relative_value = false

[[controls]]
type = "RotaryEncoder"
pin_a = 13
pin_b = 19
cc = 23
# debounce_ms is optional
relative_value = true
```

