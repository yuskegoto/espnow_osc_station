use anyhow::{bail, Result};
use log::*;
use num_derive::FromPrimitive;
use rosc::{self, OscMessage, OscPacket, OscType};

use esp_idf_hal::reset::restart;

extern crate num;
extern crate num_derive;

use std::net::{SocketAddrV4, UdpSocket, Ipv4Addr};
use std::time::Duration;

use bbqueue::framed::{FrameProducer, FrameConsumer};

const OSC_LISTEN_INTERVAL_MS: Duration = Duration::from_millis(1);
pub const MSG_BUF_DOWNSTREAM: usize = 128;
pub const MSG_BUF_UPTREAM: usize = 128;
pub const MSG_BUF_LED: usize = 4;
pub const MSG_BUF_ERROR: usize = 4;
pub const MSG_BUF_IP: usize = 32;

#[allow(dead_code)]
#[derive(FromPrimitive, Clone, Copy)]
pub enum Msg {
    Boot = 0x42,            // 'B' "Boot report"
    Mac = 0x4D,             // 'M' 'MAC report'
    Status = 0x55,          // 'U' 'statUs'

    Reset =     0x62,       // 'b' 'Reset'
    MacQuery = 0x6D,        // 'm' 'mac address query'
    Run = 0x72,             // r, Run
    StatusQuery = 0x75,     // u, statUs
}

pub struct OscReceiver {
    sock: UdpSocket,
    buf: [u8; rosc::decoder::MTU],
    sender: FrameProducer<'static, MSG_BUF_DOWNSTREAM>,
    destip_producer: FrameProducer<'static, MSG_BUF_IP>,
}

impl OscReceiver {
    pub fn new(
        ip: embedded_svc::ipv4::Ipv4Addr,
        recv_port: u16,
        sender: FrameProducer<'static, MSG_BUF_DOWNSTREAM>,
        destip_producer: FrameProducer<'static, MSG_BUF_IP>,
    ) -> Self {
        let recv_addr = SocketAddrV4::new(ip, recv_port);
        let sock = UdpSocket::bind(recv_addr).unwrap();
        let buf = [0u8; rosc::decoder::MTU];

        info!("Listening to {recv_addr}");

        Self {
            sock,
            buf,
            sender,
            destip_producer,
        }
    }

    /**
     * OSC message receiver from PC
    */
    pub fn run(&mut self) -> Result<()> {
        match self.sock.recv_from(&mut self.buf) {
            Ok((size, _addr)) => {
                info!("Received packet with size {size} from: {_addr}");

                let res = rosc::decoder::decode_udp(&self.buf[..size]);
                match res {
                    Ok((_, packet)) => {
                        match packet {
                            OscPacket::Message(msg) => {
                                info!("OSC address: {}", msg.addr);
                                info!("OSC arguments: {:?}, len:{}", msg.args, msg.args.len());

                                let device_no = if msg.args.len() > 0 {
                                    match msg.args[0] {
                                        OscType::Int(no) => {
                                            no as u8
                                        }
                                        _=> {0u8}
                                    }
                                }
                                else {0u8};
                                info!("devNo: {device_no}");

                                match msg.addr.as_str() {
                                    "/macquery" => {
                                        if msg.args.len() == 1 {
                                            self.send_downstream_buffer(Msg::MacQuery, &[device_no]);
                                        }
                                    }

                                    "/reset" => {
                                        if msg.args.len() == 1 {
                                            if device_no == 0 {
                                                // Reset!
                                                self.reset_sequence();
                                            }
                                            else {
                                                self.send_downstream_buffer(Msg::Reset, &[device_no]);
                                            }
                                        }
                                    }

                                    "/statusquery" => {
                                        if msg.args.len() == 1 {
                                            self.send_downstream_buffer(Msg::StatusQuery, &[device_no]);
                                        }
                                    }

                                    "/run" => {
                                        if msg.args.len() == 1 {
                                            self.send_downstream_buffer(Msg::Run, &[device_no]);
                                        }
                                    }

                                    "/setdestip" => {
                                        if msg.args.len() == 4 {
                                            let mut commandbuf = vec![];
                                            for arg in msg.args.iter(){
                                                let ip = arg.clone().int().unwrap();
                                                commandbuf.push((ip & 0xFF) as u8);
                                            }
                                            self.notify_new_destip(&commandbuf);
                                        }
                                    }

                                    _ => {}
                                }
                            }
                            OscPacket::Bundle(bundle) => {
                                info!("OSC Bundle: {bundle:?}");
                            }
                        }
                    }
                    Err(e) => {
                        bail!("Error receiving OSC msg: {e}");
                    }
                }
                Ok(())
            }
            Err(e) => {
                bail!("Error receiving from socket: {e}");
            }
        }
    }

    fn send_downstream_buffer(&mut self, header: Msg, content: &[u8]){
        let mut msg_buf = vec![];
        msg_buf.push(header as u8);
        for ct in content.iter(){
            msg_buf.push(*ct);
        }
        info!("Downstream buf:{:02X?}", msg_buf);

        let sz = msg_buf.len();
        if let Ok(mut wg) = self.sender.grant(sz){
            wg.to_commit(sz);
            wg.copy_from_slice(msg_buf.as_slice());
            wg.commit(sz);
        }
        else{
            error!("Downstream Buffer Overflow!");
        }
    }

    /**
     * Notify new upstream IP address
    */
    fn notify_new_destip(&mut self, newip: &[u8]){
        info!("New Dest IP:{:02X?}", newip);

        let sz = newip.len();
        if let Ok(mut wg) = self.destip_producer.grant(sz){
            wg.to_commit(sz);
            wg.copy_from_slice(newip);
            wg.commit(sz);
        }
        else{
            error!("unable to get buffer for dest ip!");
        }
    }

    /**
     * Reset the device on /reset 0 command!
    */
    fn reset_sequence(&self){
        // Wait a little bit until all buffer is cleared etc
        std::thread::sleep(Duration::from_millis(100));
        restart();
    }

    /**
     * Sleep until next interval
    */
    pub fn idle(&self) {
        std::thread::sleep(OSC_LISTEN_INTERVAL_MS);
    }

}

///////////////////////////////////////////////////////
// Upstream Messenger
// ESPNOW Receiver -> Upstream Message Buffer -> OSC Send out
pub struct OscSender {
    sock: UdpSocket,
    consumer: FrameConsumer<'static, MSG_BUF_UPTREAM>,
    dest_addr: SocketAddrV4,
    led_producer: FrameProducer<'static, MSG_BUF_LED>,
    error_msg_consumer: FrameConsumer<'static, MSG_BUF_ERROR>,
    destip_consumer: FrameConsumer<'static, MSG_BUF_IP>,
}

impl OscSender {
    pub fn new(
        dest_ip: embedded_svc::ipv4::Ipv4Addr,
        dest_port: u16,
        host_ip: embedded_svc::ipv4::Ipv4Addr,
        host_port: u16,
        consumer: FrameConsumer<'static, MSG_BUF_UPTREAM>,
        led_producer: FrameProducer<'static, MSG_BUF_LED>,
        error_msg_consumer: FrameConsumer<'static, MSG_BUF_ERROR>,
        destip_consumer: FrameConsumer<'static, MSG_BUF_IP>,
    ) -> Self {
        let dest_addr = SocketAddrV4::new(dest_ip, dest_port);
        let host_addr = SocketAddrV4::new(host_ip, host_port);
        let sock = UdpSocket::bind(host_addr).unwrap();

        Self {
            sock,
            consumer,
            dest_addr,
            led_producer,
            error_msg_consumer,
            destip_consumer,
        }
    }

    /**
     * Receives message from ESPNOW receiver, dispatches OSC message to upstream
    */
    pub fn run(&mut self) -> Result<()> {
            if let Some(frame) = self.consumer.read() {
                // Store device No
                let mut buf = vec![OscType::Int(frame[1] as i32)];
                let msg_type = num::FromPrimitive::from_u8(frame[0]);

                let addr_str = match msg_type {
                    Some(Msg::Mac) => {
                        for f in frame[2..].iter(){
                            buf.push(OscType::Int(*f as i32));
                        }
                        "/mac".to_string()
                    }

                    Some(Msg::Boot) => {
                        "/boot".to_string()
                    }

                    Some(Msg::Status) => {
                        for f in frame[2..].iter(){
                            buf.push(OscType::Int(*f as i32));
                        }
                        "/status".to_string()
                    }

                    _ => {
                        // Append header to the packet for debug
                        buf.push(OscType::Int(frame[0] as i32));
                        if frame.len() > 2 {
                            for f in frame[2..].iter(){
                                buf.push(OscType::Int(*f as i32));
                            }
                        }
                        "/unknown".to_string()
                    }
                };
                frame.release();

                // Send OSC message to PC
                info!("Send {:?} to {:?}  msg:{:X?}", addr_str, self.dest_addr, buf);
                let msg_buf =
                    rosc::encoder::encode(&OscPacket::Message(OscMessage {
                        addr: addr_str,
                        args: buf,
                    }))?;

                let ret = self.sock.send_to(&msg_buf, self.dest_addr);
                match ret {
                    Ok(_) => {
                        // Send out led1 indication
                        if let Ok(mut wg) = self.led_producer.grant(1){
                            wg.to_commit(1);
                            wg[0] = 1;
                            wg.commit(1);
                        }
                    }
                    Err(e) => {
                    bail!("Error sending out osc msg to PC1: {e}");
                    }
                }
            };

        self.check_espnow_error()?;
        self.check_dest_ip_change()?;

        Ok(())
    }

    /**
     * Sleep until next interval
    */
    pub fn idle(&self) {
        std::thread::sleep(OSC_LISTEN_INTERVAL_MS);
    }

    /**
     *  Send boot msg to the PC with my device number: 0
     */
    pub fn send_bootmsg(&self) -> Result<()>{
        let msg_buf =
        rosc::encoder::encode(&OscPacket::Message(OscMessage {
            addr: "/boot".to_string(),
            args: vec![OscType::Int(0)],
        }))?;

        if let Err(e) = self.sock.send_to(&msg_buf, self.dest_addr)
        {
            error!("Error sending OSC{e}");
        }

        Ok(())
    }

    /**
     * Check buffer for espnow error, if there is an error, send back error OSC msg to PC
    */
    fn check_espnow_error(&mut self) -> Result<()>{
        if let Some(frame) = self.error_msg_consumer.read()
        {
            let dev_no = frame[0];
            frame.release();

            let msg_buf =
            rosc::encoder::encode(&OscPacket::Message(OscMessage {
                addr: "/notfound".to_string(),
                args: vec![OscType::Int(dev_no as i32)],
            }))?;

            if let Err(e) = self.sock.send_to(&msg_buf, self.dest_addr)
            {
                error!("Error sending OSC{e}");
            }
        }
        Ok(())
    }

    fn check_dest_ip_change(&mut self) -> Result<()>{
        if let Some(frame) = self.destip_consumer.read()
        {
            if frame.len() == 4 {
                let mut newip = [0u8; 4];
                newip.copy_from_slice(&frame);
                let dest_ip = Ipv4Addr::from(newip);
                self.dest_addr.set_ip(dest_ip);
            }
            frame.release();

            let ip_addr = self.dest_addr.ip().octets();
            let msg_buf =
            rosc::encoder::encode(&OscPacket::Message(OscMessage {
                addr: "/destip".to_string(),
                args: vec![OscType::Int(ip_addr[0] as i32), OscType::Int(ip_addr[1] as i32), OscType::Int(ip_addr[2] as i32), OscType::Int(ip_addr[3] as i32)],
            }))?;

            if let Err(e) = self.sock.send_to(&msg_buf, self.dest_addr)
            {
                error!("Error sending OSC{e}");
            }
        }
        Ok(())
    }

}