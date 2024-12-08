#![no_std]
#![no_main]

use core::fmt::{self, Write as _};
use cortex_m::singleton;
use cyw43::{Control, JoinOptions};
use cyw43_pio::PioSpi;
use defmt::{debug, info, warn};
use embassy_executor;
use embassy_executor::Spawner;
use embassy_futures::select::{select, select3};
use embassy_futures::yield_now;
use embassy_net::tcp::TcpSocket;
use embassy_net::Ipv4Address;
use embassy_net::Runner;
use embassy_net::{Config as NetConfig, Stack};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{AnyPin, Level, Output};
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_time::{Duration, Timer};
use heapless;
use httparse::{self, Request};
use static_cell::StaticCell;
use wifi_clock::blob;
use wifi_clock::dhcp_server::{DhcpServer, DhcpServerConfig};

use {defmt_rtt as _, panic_probe as _};

mod clock;
mod display;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

struct WriteBuf<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> WriteBuf<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    pub fn as_slice(&'a mut self) -> &'a mut [u8] {
        &mut self.buf[..self.len]
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> fmt::Result {
        let left = self.buf.len() - self.len;
        let copy = bytes.len();
        if left >= copy {
            self.buf[self.len..(self.len + copy)].copy_from_slice(bytes);
            self.len += copy;
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

impl<'a> fmt::Write for WriteBuf<'a> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_bytes(s.as_bytes())
    }
}

async fn handle_request<'a>(
    req: &'a Request<'a, 'a>,
    status_code: &mut u32,
    content_type: &mut &str,
    body: &mut &[u8],
    led_on: &'a mut bool,
    clock: &'a mut clock::ClockControl,
) {
    if let Some("GET") = req.method {
        if let Some(path) = req.path {
            if let Some(cmd) = path.strip_prefix("/cmd/") {
                *status_code = 204;
                *body = &[0; 0];
                if cmd.starts_with("on") {
                    *led_on = true;
                } else if cmd.starts_with("off") {
                    *led_on = false;
                } else if cmd.starts_with("start") {
                    clock.stopwatch_start().await;
                } else if cmd.starts_with("stop") {
                    clock.stopwatch_stop().await;
                } else if cmd.starts_with("reset") {
                    clock.stopwatch_reset().await;
                }
                /*else if let Some(args) = cmd.strip_prefix("write?") {
                let args = args.split('&');
                let mut addr = 0;
                let mut data = 0;
                for arg in args {
                if let Some(addr_str) = arg.strip_prefix("addr=") {
                    addr = str::parse::<u8>(addr_str).unwrap_or(0);
                }else if let Some(data_str) = arg.strip_prefix("data=") {
                    data = str::parse::<u8>(data_str).unwrap_or(0)
                }
                }
                info!("Addr: {}", addr);
                info!("Data: {}", data);
                led_bus.write_data(addr, data);
                    } */
                else {
                    *status_code = 400;
                    *body =
			"<html><head><title>Illegal request</title></head><body>400 Unknown command</body></html>".as_bytes();
                }
            } else if path.starts_with("/index.html") || path == "/" {
                *status_code = 200;
                *body = include_bytes!("index.html");
            } else if path.starts_with("/style.css") {
                *status_code = 200;
                *body = include_bytes!("style.css");
                *content_type = "text/css";
            }
        }
    } else {
        *status_code = 400;
        *body =
			"<html><head><title>Illegal request</title></head><body>400 Only GET allowed</body></html>".as_bytes();
    }
}
#[embassy_executor::task]
async fn net_task(runner: &'static mut Runner<'static, cyw43::NetDriver<'static>>) -> ! {
    runner.run().await
}
const MAX_TX_BLOCK: usize = 1024;
const MAX_RX_BLOCK: usize = 1024;

async fn http_task<D>(
    stack: Stack<'static>,
    mut control: Control<'static>,
    mut clock: clock::ClockControl,
) where
    D: embassy_net::driver::Driver,
{
    control.gpio_set(0, true).await;

    let rx_buffer = singleton!(: [u8; MAX_RX_BLOCK] = [0; MAX_RX_BLOCK]).unwrap();
    let tx_buffer = singleton!(: [u8; MAX_TX_BLOCK] = [0; MAX_TX_BLOCK]).unwrap();
    let buf = singleton!(: [u8; 4096] = [0; 4096]).unwrap();
    let resp = singleton!(: [u8; 8192] = [0; 8192]).unwrap();
    let mut led_on = true;
    loop {
        let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(60)));

        info!("Listening on TCP:80...");
        if let Err(e) = socket.accept(80).await {
            warn!("accept error: {:?}", e);
            continue;
        }

        info!("Received connection from {:?}", socket.remote_endpoint());
        let mut buf_end: usize = 0;

        loop {
            let n = match socket.read(&mut buf[buf_end..]).await {
                Ok(0) => {
                    warn!("read EOF");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    warn!("read error: {:?}", e);
                    break;
                }
            };

            buf_end += n;
            info!("Buffer size: {}", buf_end);

            let mut headers = [httparse::EMPTY_HEADER; 20];
            let mut req = httparse::Request::new(&mut headers);
            let res = match req.parse(buf) {
                Ok(res) => res,
                Err(_) => {
                    warn!("Parsing request failed");
                    socket.close();
                    continue;
                }
            };
            if res.is_complete() {
                let mut status_code: u32 = 404;
                let mut content_type = "text/html;charset=UTF-8";
                let mut body =
                    "<html><head><title>Not found</title></head><body>404 Not found</body></html>"
                        .as_bytes();
                info!("Method: {:?}", req.method);
                handle_request(
                    &req,
                    &mut status_code,
                    &mut content_type,
                    &mut body,
                    &mut led_on,
                    &mut clock,
                )
                .await;
                buf_end = 0;
                control.gpio_set(0, led_on).await;
                let mut resp_writer = WriteBuf::new(resp);

                write!(
                    resp_writer,
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: {}\r\n\r\n",
                    status_code,
                    body.len(),
                    content_type,
                )
                .unwrap();

                resp_writer.write_bytes(body).unwrap();
                let mut tx_block: &[u8] = resp_writer.as_slice();
                while !tx_block.is_empty() {
                    let send_block = if tx_block.len() > MAX_TX_BLOCK {
                        &tx_block[..MAX_TX_BLOCK]
                    } else {
                        tx_block
                    };
                    match socket.write(send_block).await {
                        Ok(_wlen) => {}
                        Err(e) => {
                            warn!("write error: {:?}", e);
                            break;
                        }
                    }
                    debug!("Sent: {}", send_block.len());
                    tx_block = &tx_block[send_block.len()..]
                }
            }
        }
    }
}

async fn wait_for_config<D>(stack: &'static Stack<'static>) -> embassy_net::StaticConfigV4
where
    D: embassy_net::driver::Driver,
{
    loop {
        if let Some(config) = stack.config_v4() {
            return config.clone();
        }
        yield_now().await;
    }
}

#[embassy_executor::task]
async fn setup_task(
    spawner: Spawner,
    mut control: Control<'static>,
    net_device: cyw43::NetDriver<'static>,
    clock: clock::ClockControl,
) {
    let clm = blob::cyw_43439a0_clm();
    control.init(clm).await;
    let config;
    let ssid = option_env!("SSID");
    let pass = option_env!("PASS");
    if let (Some(ssid), Some(pass)) = (ssid, pass) {
        info!("Joining");
	let options = JoinOptions::new(pass.as_bytes());
        loop {
            match control.join(ssid, options.clone()).await {
                Ok(_) => break,
                Err(err) => {
                    info!("join failed with status={}", err.status);
                }
            }
        }
        config = NetConfig::dhcpv4(Default::default());
    } else {
        config = NetConfig::ipv4_static(embassy_net::StaticConfigV4 {
            address: embassy_net::Ipv4Cidr::new(embassy_net::Ipv4Address::new(192, 168, 17, 1), 24),
            dns_servers: heapless::Vec::new(),
            gateway: None,
        });
        control.start_ap_wpa2("Clock", "password", 5).await;
    }
    let seed = 63395997077266;
    static RESOURCES: StaticCell<embassy_net::StackResources<3>> = StaticCell::new();
    let (stack, runner) = singleton!(:(Stack<'static>, Runner<'static, cyw43::NetDriver>)= embassy_net::new(
        net_device,
        config,
        RESOURCES.init(embassy_net::StackResources::new()),
        seed
    ))
    .unwrap();
    spawner.spawn(net_task(runner)).unwrap();

    wait_for_config::<cyw43::NetDriver>(stack).await;
    info!("Done");
    const DHCP_SERVER_CONFIG: DhcpServerConfig = DhcpServerConfig {
        subnet_mask: Ipv4Address::new(255, 255, 255, 0),
        router: Some(Ipv4Address::new(192, 168, 17, 1)),
        lease_duration: 60 * 10,
    };
    let mut dhcp_server =
        DhcpServer::<4>::new(Ipv4Address::new(192, 168, 17, 151), DHCP_SERVER_CONFIG);
    select(http_task::<cyw43::NetDriver>(*stack, control, clock), dhcp_server.run(*stack)).await;
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let wl_on = Output::new(p.PIN_23, Level::Low);
    Timer::after(Duration::from_millis(150)).await;

    let fw = blob::cyw_43439a0();

    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );
    //let cs = Output::new(p.PIN_2, Level::High);
    let state = singleton!(: cyw43::State = cyw43::State::new()).unwrap();

    // LED control pins
    let d_pins: [Output; 8] = [
        Output::new(AnyPin::from(p.PIN_0), Level::High),
        Output::new(AnyPin::from(p.PIN_1), Level::High),
        Output::new(AnyPin::from(p.PIN_2), Level::High),
        Output::new(AnyPin::from(p.PIN_3), Level::High),
        Output::new(AnyPin::from(p.PIN_4), Level::High),
        Output::new(AnyPin::from(p.PIN_5), Level::High),
        Output::new(AnyPin::from(p.PIN_6), Level::High),
        Output::new(AnyPin::from(p.PIN_7), Level::High),
    ];
    let en_pin = Output::new(AnyPin::from(p.PIN_9), Level::Low);
    let as_pin = Output::new(AnyPin::from(p.PIN_10), Level::Low);
    let wr_pin = Output::new(AnyPin::from(p.PIN_8), Level::High);

    let led_bus = display::LedBus::new(d_pins, en_pin, as_pin, wr_pin);
    let (disp, disp_runner) = display::new(led_bus);
    disp.set_int(0..4, 1927).await;

    let (clock, clock_runner) = clock::new(disp);
    info!("Initializing");
    let (net_device, control, runner) = cyw43::new(state, wl_on, spi, fw).await;
    spawner
        .spawn(setup_task(spawner, control, net_device, clock))
        .unwrap();
    select3(runner.run(), disp_runner, clock_runner).await;
}
