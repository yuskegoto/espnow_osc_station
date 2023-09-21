# About
- Rust project to bridge ethernet OSC commands to 2.4GHz short message protocol, ESP-NOW.

## Why?
I have been building a lot of exhibitions and one-off entertainent systems, and I had to use WiFi a lot,
eventhough 2.4GHz WiFi (and BLE) band is extremely congested especially in crowded location, like exibition hall.

The [ESP-NOW](https://www.espressif.com/en/solutions/low-power-solutions/esp-now) is a wireless protocol developped by Espressif, which uses 2.4GHz and utilizes WiFi's physical (and data link) stack.
The pros of the ESP-NOW are that once bonded, it's (relatively) reliable, can check packet delivery and able to communicate with multiple devices.
The downside of the protocol is that you can't diectly communicate with PCs. This is why I develop this project.

## Related projects
[ESP-NOW Arduino Client]()
[ESP-NOW Rust Client](https://github.com/yuskegoto/espnow_rust_client)

## Hardware
I target [T-Internet-POE](https://www.lilygo.cc/products/t-internet-poe) for this project because it was cheap to obtain. It uses Ethernet-PHY LAN8720A and can accept PoE (with some cautions!).
I have noticed when the device is powered up via PoE on long cable, it may hangup time-to-time, presumably due to power drop caused by the power surge on sending out ESP-NOW packet.
Also note that this board needs additional USB-UART board.

Olimex or the original ESP32-Etehrnet-Kit would be other alternatives but have't tested yet. (Feel free to send me a sample if you want me to test them!)

## Project
The project is written on Rust for EPS32.
There is also a sample projects for MAX8.
I'll post client side project for Arduino and Rust.

# Functions
- OSC and ESP-NOW receive indicators.
- Error message when the ESP-NOW message not reached.
- Retry on unsuccesful ESP-NOW derivery.
- Configureable OSC upstream IP address via /setdestip command.
- Multiple ESP-NOW bridges can coexists to build a resilient system.

# Setting up environment
For details and newest info please refer [The Rust on ESP Book](https://esp-rs.github.io/book/installation/index.html)
* Install Rust
`
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
`
* Install esp-rs tool chain
`
cargo install espup
espup install
cargo install cargo-generate cargo-espflash espflash cargo-espmonitor espmonitor ldproxy
`

## Configs
- IP addresses and OSC ports can be set via environment variables.
```PowerShell
$env:OSC_RECV_PORT = '5000'
$env:OSC_SEND_PORT = '5101'
$env:OSC_DEST_PORT = '5101'
$env:OSC_LOCAL_IP = '192.168.1.10'
$env:OSC_DEST_IP = '192.168.1.20'
$env:OSC_GATEWAY_IP = '192.168.1.1'
$env:ESPNOW_CHANNEL = '0'
```
- Device MAC addresses can be set in espnow.rs

## Build Commands
```PowerShell
cargo run
~/export-esp.ps1
espflash board-info
espmonitor <OCM_PORT_NO>
```
```bash
export OSC_RECV_PORT_STR=5000
```

## Build / Run in offline mode
cargo build --offline

# Protocol
## OSC Structure
`
[OSC Address] [Device No] [Packet]
/setdestip 0 192 168 1 10

#Send /run command to device number 1 with 10 value
/run 1 10
`
## ESP-NOW Packet structure
|Header|Device No|Packet|
|0x72|0x01|0x0A|

## To add message
- Add Msg enum in osc.rs
- 

## Crate
- [rosc](https://crates.io/crates/rosc) is used to encode/decode OSC packet
- Using [bbqueue](https://docs.rs/bbqueue/latest/bbqueue/) for between threads communiations.


## References
- [Rust-ESP32-STD-demo](https://github.com/ivmarkov/rust-esp32-std-demo/blob/main/src/main.rs)
- [ESP-NOW Rust sample](https://github.com/esp-rs/esp-wifi/blob/main/examples-esp32/examples/esp_now.rs)
- Tai Hideaki san's [rust-esp32-osc-led](https://github.com/hideakitai/rust-esp32-osc-led.git)
