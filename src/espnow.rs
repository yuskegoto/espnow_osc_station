use anyhow::{bail, Result};
use std::time::Duration;
use bbqueue::framed::{FrameConsumer, FrameProducer};
use log::*;

extern crate num;
extern crate num_derive;

use esp_idf_sys::{self as _, esp_interface_t_ESP_IF_WIFI_AP}; // If using the `binstart` feature of `esp-idf-sys`, always keep this module imported
use esp_idf_svc::espnow::*;

use crate::{PRODUCER_UPSTREAM, MSG_BUF_DOWNSTREAM, MSG_BUF_LED, PRODUCER_SENDERROR, MSG_BUF_ESPNOWRETRY, ESPNOW_RETRY_COUNT, PRODUCER_ESPNOWRETRY, ESPNOW_MAX_RETRY, ESPNOW_LAST_PACKET, ESPNOW_LAST_PACKET_LENGTH};

const ESPNOW_FRAME_INTERVAL_MS: Duration = Duration::from_millis(1);

// Adding device's MAC address to NODE_ADDRESSES const
const DEV1_MAC: [u8;6] = [0x50, 0x02, 0x91, 0x9F, 0xCF, 0x9C];
const DEV2_MAC: [u8;6] = [0x50, 0x02, 0x91, 0x87, 0x95, 0x81];
const NODE_ADDRESSES: [[u8;6]; 3] = [BROADCAST, DEV1_MAC, DEV2_MAC];

pub struct Espnow{
    receiver: FrameConsumer<'static, MSG_BUF_DOWNSTREAM>,
    led_producer: FrameProducer<'static, MSG_BUF_LED>,
    espnow_retry_cosumer: FrameConsumer<'static, MSG_BUF_ESPNOWRETRY>,
    espnow: EspNow,
}

impl Espnow{
    pub fn new(receiver: FrameConsumer<'static, MSG_BUF_DOWNSTREAM>, led_producer: FrameProducer<'static, MSG_BUF_LED>,
        espnow_retry_cosumer: FrameConsumer<'static, MSG_BUF_ESPNOWRETRY>) -> Self {
        let espnow = EspNow::take().unwrap();
        let _ = espnow.register_recv_cb(recv_callback).unwrap();
        let _ = espnow.register_send_cb(send_callback).unwrap();
        Self {
            receiver,
            led_producer,
            espnow_retry_cosumer,
            espnow,
        }
    }

    /**
     * Adding peer addresses to peer list
    */
    pub fn config(&mut self, peer_channel: u8){

        for peer_addr in NODE_ADDRESSES{
            let peer_info = PeerInfo {
                peer_addr: peer_addr,
                lmk: [0u8; 16],
                channel: peer_channel,
                encrypt: false,
                ifidx: esp_interface_t_ESP_IF_WIFI_AP,
                priv_: std::ptr::null_mut(),
            };
            if let Err(e) = self.espnow.add_peer(peer_info){
                error!("ESPNOW add peer error: {e}");
            };
        };
    }

    /**
     * On receiving OSC packet, send out ESPnow.
    */
    pub fn run(&mut self) -> Result<()> {
        if let Some(frame) = self.receiver.read() {
            info!("downstream msg received");

            let mut data = [0u8; 10];
            data[..frame.len()].copy_from_slice(&frame);
            let target_no = data[1] as usize;

            if NODE_ADDRESSES.len() > target_no {
                let ret = self.espnow.send(NODE_ADDRESSES[target_no], &data[..frame.len()]);
                match ret {
                    Ok(_) => {
                        // Send out led indication
                        if let Ok(mut wg) = self.led_producer.grant(1){
                            wg.to_commit(1);
                            wg[0] = 1;
                            wg.commit(1);
                        }
                        unsafe{
                            if frame.len() < 10 {
                                ESPNOW_LAST_PACKET[..frame.len()].copy_from_slice(&data[..frame.len()]);
                                ESPNOW_LAST_PACKET_LENGTH = frame.len();
                            }
                        }
                    }
                    Err(e) => {
                    bail!("Error sending out espnow msg: {e}");
                    }
                }
            }
            else {
                error!("This device does not exists! {target_no}");
            }

            frame.release();
            // frame.auto_release(true);
        }
        Ok(())
    }

    
    /**
     * When ESPNOW send is failed, retry.
    */
    pub fn send_retry(&mut self) -> Result<()> {
        if let Some(frame) = self.espnow_retry_cosumer.read() {
            // info!("ESPNOW: retry");

            frame.release();

            let mut data = [0u8; 10];
            unsafe{
                if ESPNOW_LAST_PACKET_LENGTH < 10 {
                    data[..ESPNOW_LAST_PACKET_LENGTH].copy_from_slice(&ESPNOW_LAST_PACKET[..ESPNOW_LAST_PACKET_LENGTH]);
                }
            };
            let data_len = unsafe{ESPNOW_LAST_PACKET_LENGTH};

            let target_no = data[1] as usize;

            self.send_msg(target_no, &data[..data_len])?;
        }
        Ok(())
    }

    fn send_msg(&mut self, target_no:usize, data:&[u8]) -> Result<()> {
        if NODE_ADDRESSES.len() > target_no {
            let ret = self.espnow.send(NODE_ADDRESSES[target_no], data);
            match ret {
                Ok(_) => {
                    // Send out led indication
                    if let Ok(mut wg) = self.led_producer.grant(1){
                        wg.to_commit(1);
                        wg[0] = 1;
                        wg.commit(1);
                    }
                }
                Err(e) => {
                bail!("Error sending out espnow msg: {e}");
                }
            }
        }
        else {
            error!("This device does not exists! {target_no}");
        }
        Ok(())
    }

    /**
     * Sleep the thread until next interval
    */
    pub fn idle(&self) {
        std::thread::sleep(ESPNOW_FRAME_INTERVAL_MS);
    }
}

/**
 * ESPnow message callback
 * Messages are simply forwarded to UPSTREAM buffer, will be handled in OscSender
*/
fn recv_callback(_recv_info:&[u8], data:&[u8]){
    info!("espnow:recv_info:{:X?}, data:{:X?}", _recv_info, data);
    unsafe {if PRODUCER_UPSTREAM.is_some(){
        let producer = PRODUCER_UPSTREAM.as_mut().unwrap();
        let sz = data.len();
        if let Ok(mut wg) = producer.grant(sz){
            wg.to_commit(sz);
            wg.copy_from_slice(data);
            wg.commit(sz);
        }
        else{
            error!("ESPNOW:UpStream Buffer Overflow!");
        }

    }}
}

/**
 * ESPnow send callback. When espnow send is completed, determine the SendStatus
 * and give back the error osc message when the espnow messge did not reach to destination.
*/
fn send_callback(mac_addr:&[u8], send_status: SendStatus){
    match send_status {
        SendStatus::SUCCESS => {
            info!("send to {:X?} succesfull", mac_addr);
            unsafe{ESPNOW_RETRY_COUNT = 0;}
        }
        SendStatus::FAIL => {
            error!("ESPNOW:sending to {:X?} failed!", mac_addr);

            // Find device no
            let mut dev_no:u8 = 0;
            for (i, n) in NODE_ADDRESSES.iter().enumerate(){
                if n == mac_addr{
                    dev_no = i as u8;
                    break;
                }
            }

            if dev_no != 0{
                // Retry
                unsafe {
                    if ESPNOW_RETRY_COUNT < ESPNOW_MAX_RETRY{
                        if PRODUCER_ESPNOWRETRY.is_some(){
                            ESPNOW_RETRY_COUNT += 1;
                            let producer = PRODUCER_ESPNOWRETRY.as_mut().unwrap();
                            if let Ok(mut wg) = producer.grant(1){
                                wg.to_commit(1);
                                wg[0] = dev_no;
                                wg.commit(1);
                            }
                        }
                    }
                    // Send error on retry failure
                    else {
                        if PRODUCER_SENDERROR.is_some(){
                            let producer = PRODUCER_SENDERROR.as_mut().unwrap();
                            if let Ok(mut wg) = producer.grant(1){
                                wg.to_commit(1);
                                wg[0] = dev_no;
                                wg.commit(1);
                            }
                        }
                    }
                }
            }

        }
    }
}