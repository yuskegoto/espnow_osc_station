use anyhow::*;
use std::result::Result::Ok;
use log::*;

use std::time::Duration;

use esp_idf_sys::{self as _}; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported

use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition, netif};
use embedded_svc::wifi::{AuthMethod, Configuration, AccessPointConfiguration};
use embedded_svc::ipv4;
use embedded_svc::ipv4::{ClientConfiguration, ClientSettings, Subnet, Mask};

use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::gpio;

#[cfg(feature = "W5500")]
use esp_idf_hal::spi;

use esp_idf_hal::gpio::*;

use std::net::Ipv4Addr;
use std::str::FromStr;

use bbqueue::BBBuffer;
use bbqueue::framed::FrameProducer;

mod osc;
use osc::{*, MSG_BUF_UPTREAM, MSG_BUF_DOWNSTREAM, MSG_BUF_LED, MSG_BUF_ERROR, MSG_BUF_IP};

mod espnow;
use espnow::Espnow;

static QUEUE_DOWNSTREAM: BBBuffer<MSG_BUF_DOWNSTREAM>= BBBuffer::new();
static QUEUE_UPSTREAM: BBBuffer<MSG_BUF_UPTREAM>= BBBuffer::new();
static QUEUE_LED: BBBuffer<MSG_BUF_LED>= BBBuffer::new();
static QUEUE_LED1: BBBuffer<MSG_BUF_LED>= BBBuffer::new();
static QUEUE_ERROR: BBBuffer<MSG_BUF_ERROR>= BBBuffer::new();
static QUEUE_DEST_IP: BBBuffer<MSG_BUF_IP>= BBBuffer::new();

static QUEUE_ESPNOWRETRY: BBBuffer<MSG_BUF_ESPNOWRETRY>= BBBuffer::new();
static mut ESPNOW_RETRY_COUNT:usize = 0;
static mut ESPNOW_LAST_PACKET: [u8;10] = [0u8;10];
static mut ESPNOW_LAST_PACKET_LENGTH: usize = 0;
pub const MSG_BUF_ESPNOWRETRY: usize = 4;
const ESPNOW_MAX_RETRY: usize = 3;

static mut PRODUCER_UPSTREAM: Option<FrameProducer<MSG_BUF_UPTREAM>> = None;
static mut PRODUCER_SENDERROR: Option<FrameProducer<MSG_BUF_ERROR>> = None;
static mut PRODUCER_ESPNOWRETRY: Option<FrameProducer<MSG_BUF_ERROR>> = None;

static LED_SLEEP_DURATION_MS: Duration = Duration::from_millis(50);

#[allow(dead_code)]
const RECV_PORT_STR: &str = env!("OSC_RECV_PORT");
// const RECV_PORT: u16 = 5000;

#[allow(dead_code)]
const SEND_PORT_STR: &str = env!("OSC_SEND_PORT");
// const SEND_PORT: u16 = 5101;

#[allow(dead_code)]
const DEST_IP: &str = env!("OSC_DEST_IP");
// const DEST_IP: &str = "192.168.1.20";
// const DEST_IP2: &str = "192.168.1.21";

#[allow(dead_code)]
const GATEWAY_IP: &str = env!("OSC_GATEWAY_IP");
// const GATEWAY_IP: &str = "192.168.1.1";

#[allow(dead_code)]
const DEST_PORT_STR: &str = env!("OSC_DEST_PORT");
// const DEST_PORT: u16 = 5101;

#[allow(dead_code)]
const LOCAL_IP: &str = env!("OSC_LOCAL_IP");
// const LOCAL_IP: &str = "192.168.1.10";

#[allow(dead_code)]
const PEER_CHANNEL_STR: &str = env!("ESPNOW_CHANNEL");
// const PEER_CHANNEL: u8 = 0u8;

fn main()-> Result<()> {
    esp_idf_sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    unsafe{
        esp_idf_sys::nvs_flash_init();
    }
    let nvs = EspDefaultNvsPartition::take().unwrap();
    let sysloop = EspSystemEventLoop::take().unwrap();

    // Pin Config
    let peripherals = Peripherals::take().unwrap();
    // let button = PinDriver::input(peripherals.pins.gpio0)?;
    let mut led = PinDriver::output(peripherals.pins.gpio14)?;
    let mut led1 = PinDriver::output(peripherals.pins.gpio32)?;
    led.set_low()?;
    led1.set_low()?;

    // Wifi / ESPNow setting
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs)).unwrap(),
        sysloop.clone(),
    ).unwrap();

    let wifi_configuration: Configuration = Configuration::AccessPoint(AccessPointConfiguration{
        ssid: "espnow".into(),
        ssid_hidden: true,
        channel: 0,
        auth_method: AuthMethod::None,
        ..Default::default()
    }) ;
    wifi.set_configuration(&wifi_configuration)?;
    wifi.start()?;
    info!("Is Wifi started? {:?}", wifi.is_started());

    wifi.wait_netif_up()?;
    info!("Wifi netif up");

    let mac = wifi.wifi().ap_netif().get_mac()?;
    info!("mac address: {:X?}", mac);

    // Ethernet Config
    let local_ip = Ipv4Addr::from_str(LOCAL_IP)?;
    let gateway_ip = Ipv4Addr::from_str(GATEWAY_IP)?;

    #[cfg(feature = "LAN870")]
    let eth_driver =
        esp_idf_svc::eth::EthDriver::new_rmii(
            peripherals.mac,
            peripherals.pins.gpio25,
            peripherals.pins.gpio26,
            peripherals.pins.gpio27,
            peripherals.pins.gpio23,
            peripherals.pins.gpio22,
            peripherals.pins.gpio21,
            peripherals.pins.gpio19,
            peripherals.pins.gpio18,
            esp_idf_svc::eth::RmiiClockConfig::<gpio::Gpio0, gpio::Gpio16, gpio::Gpio17>::OutputInvertedGpio17(
                peripherals.pins.gpio17),
            Some(peripherals.pins.gpio5),
            esp_idf_svc::eth::RmiiEthChipset::LAN87XX,
            None,
            sysloop.clone(),
        )?;

    #[cfg(feature = "W5500")]
    let eth = {
        let mut eth = Box::new(esp_idf_svc::eth::EspEth::wrap(

            esp_idf_svc::eth::EthDriver::new_spi(

                spi::SpiDriver::new(
                    peripherals.spi3,
                    pins.gpio5,
                    pins.gpio19,
                    Some(pins.gpio18),
                    &spi::SpiDriverConfig::new().dma(spi::Dma::Auto(4096)),
                )?,
                pins.gpio12,
                Some(pins.gpio33),
                Some(pins.gpio21),
                esp_idf_svc::eth::SpiEthChipset::W5500,
                20.MHz().into(),
                Some(&[0x02, 0x00, 0x00, 0x12, 0x34, 0x56]),
                None,
                sysloop.clone(),
            )?,
        )?);
    };


    let mut eth_config = netif::NetifConfiguration::eth_default_client();
    let ipconfig = ipv4::Configuration::Client(
        ClientConfiguration::Fixed(
            ClientSettings {
            ip: local_ip,
            subnet: Subnet {
                gateway: gateway_ip,
                mask: Mask(24),
            },
            dns: None,
            secondary_dns: None,
            },
        ));
    eth_config.ip_configuration = ipconfig;
    let eth_netif = netif::EspNetif::new_with_conf(&eth_config)?;
    let mut eth = Box::new(
        esp_idf_svc::eth::EspEth::wrap_all(eth_driver, eth_netif)?
    );
    let local_ip = eth_configure(&sysloop, &mut eth)?;

    info!("ESPNOW Bridge started");

    let dest_ip = Ipv4Addr::from_str(DEST_IP)?;
    // let dest_ip2 = Ipv4Addr::from_str(DEST_IP2)?;

    // Thread communication buffers!
    let (downstream_msg_producer, downstream_msg_consumer) = QUEUE_DOWNSTREAM.try_split_framed().unwrap();
    let (upstream_msg_producer, upstream_msg_consumer) = QUEUE_UPSTREAM.try_split_framed().unwrap();
    unsafe {PRODUCER_UPSTREAM = Some(upstream_msg_producer);}
    let (led_msg_producer, mut led_msg_consumer) = QUEUE_LED.try_split_framed().unwrap();
    let (led1_msg_producer, mut led1_msg_consumer) = QUEUE_LED1.try_split_framed().unwrap();

    let (send_error_msg_producer, send_error_msg_consumer) = QUEUE_ERROR.try_split_framed().unwrap();
    unsafe {PRODUCER_SENDERROR = Some(send_error_msg_producer);}

    let (espnow_retry_msg_producer, espnow_retry_msg_consumer) = QUEUE_ESPNOWRETRY.try_split_framed().unwrap();
    unsafe {PRODUCER_ESPNOWRETRY = Some(espnow_retry_msg_producer);}

    let (destip_msg_producer, destip_msg_consumer) = QUEUE_DEST_IP.try_split_framed().unwrap();

    let recv_port = RECV_PORT_STR.parse::<u16>().unwrap();
    let send_port = SEND_PORT_STR.parse::<u16>().unwrap();
    let dest_port = DEST_PORT_STR.parse::<u16>().unwrap();
    let peer_channel = PEER_CHANNEL_STR.parse::<u8>().unwrap();

    // Create thread to handle ESPNow messages
    let espnow_join_handle = std::thread::Builder::new()
        .stack_size(4096)
        .spawn(move || {
            let mut espnow = Espnow::new(downstream_msg_consumer, led_msg_producer, espnow_retry_msg_consumer);
            espnow.config(peer_channel);

            loop {
                if let Err(e) = espnow.run() {
                    error!("Failed to send espnow messages: {e}");
                }
                if let Err(e) = espnow.send_retry() {
                    error!("Failed to run ESPNOW resend: {e}");
                }
                espnow.idle();
            }
        })?;

        let osc_receiver_join_handle = std::thread::Builder::new()
        .stack_size(8192)
        .spawn(move || {
            let mut osc = OscReceiver::new(local_ip, recv_port, downstream_msg_producer, destip_msg_producer);
            loop {
                if let Err(e) = osc.run() {
                        error!("Failed to run OSC: {e}");
                        // break;
                    }
                osc.idle();
            }
        })?;

    let osc_sender_join_handle = std::thread::Builder::new()
        .stack_size(8192)
        .spawn(move || {
            let mut osc_sender = OscSender::new(dest_ip, dest_port, local_ip, send_port, upstream_msg_consumer
            // let mut osc_sender = OscSender::new(dest_ip, dest_ip2, DEST_PORT, local_ip, SEND_PORT, upstream_msg_consumer
                , led1_msg_producer, send_error_msg_consumer, destip_msg_consumer);
            osc_sender.send_bootmsg().unwrap();
            loop {
                if let Err(e) = osc_sender.run() {
                        error!("Failed to run OSC Sender: {e}");
                        // break;
                    }
                osc_sender.idle();
            }
        })?;

    let led_join_handle = std::thread::Builder::new()
        .stack_size(1024)
        .spawn(move || {
            loop {
                if let Some(frame) = led_msg_consumer.read() {
                    frame.release();
                    let _ = led.set_high();
                    std::thread::sleep(LED_SLEEP_DURATION_MS);
                    let _ = led.set_low();
                }
                std::thread::sleep(LED_SLEEP_DURATION_MS);
            }
        })?;

    let led1_join_handle = std::thread::Builder::new()
        .stack_size(1024)
        .spawn(move || {
            loop {
                if let Some(frame) = led1_msg_consumer.read() {
                    frame.release();
                    let _ = led1.set_high();
                    std::thread::sleep(LED_SLEEP_DURATION_MS);
                    let _ = led1.set_low();
                }
                std::thread::sleep(LED_SLEEP_DURATION_MS);
            }
        })?;

    espnow_join_handle.join().unwrap();
    osc_receiver_join_handle.join().unwrap();
    osc_sender_join_handle.join().unwrap();
    led_join_handle.join().unwrap();
    led1_join_handle.join().unwrap();

    info!("Finish app");
    Ok(())
}

fn eth_configure<'d, T>(
    sysloop: &EspSystemEventLoop,
    eth: &mut esp_idf_svc::eth::EspEth<'d, T>,
) -> Result<Ipv4Addr> {
    info!("Eth created");
    let mut eth = esp_idf_svc::eth::BlockingEth::wrap(eth, sysloop.clone())?;
    eth.start()?;

    info!("Waiting for netif up...");

    eth.wait_netif_up()?;

    let ip_info = eth.eth().netif().get_ip_info()?;

    info!("Eth info: {:?}", ip_info);
    Ok(ip_info.ip)
}