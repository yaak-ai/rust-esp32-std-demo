use esp_idf_hal::delay;
use esp_idf_hal::gpio;
use esp_idf_hal::prelude::*;
use esp_idf_hal::uart::UartTxDriver;
use esp_idf_hal::uart::*;

use esp_idf_hal::uart::UartDriver;
use std::sync::mpsc::TryRecvError;
use std::thread;
use std::time::Duration;

use messages::*;
use std::sync::{
    mpsc::{channel, Receiver, Sender},
    Arc, Mutex,
};

use crate::uart_update;

#[derive(Debug)]
pub enum Error {
    StdIo(std::io::Error),
    Esp(esp_idf_sys::EspError),
    Anyhow(anyhow::Error),
    TryRecv(TryRecvError),
    Other(String),
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::StdIo(err)
    }
}

impl From<esp_idf_sys::EspError> for Error {
    fn from(err: esp_idf_sys::EspError) -> Self {
        Error::Esp(err)
    }
}

impl From<anyhow::Error> for Error {
    fn from(err: anyhow::Error) -> Self {
        Error::Anyhow(err)
    }
}
impl From<TryRecvError> for Error {
    fn from(err: TryRecvError) -> Self {
        Error::TryRecv(err)
    }
}

pub fn serial_ota() -> std::result::Result<(), Error> {
    thread::Builder::new()
        .name("Serial thread".to_string())
        .stack_size(25 * 1024)
        .spawn(move || {
            let peripherals = Peripherals::take().unwrap();

            let config = config::Config::new().baudrate(Hertz(115_200));
            let uart = UartDriver::new(
                peripherals.uart1,
                peripherals.pins.gpio17,
                peripherals.pins.gpio16,
                Option::<gpio::Gpio0>::None,
                Option::<gpio::Gpio1>::None,
                &config,
            )
            .unwrap();

            let (serial_tx, serial_rx): (Sender<Message<MessageTypeMcu>>, _) = channel();
            let serial_rx = Arc::new(Mutex::new(serial_rx));
            let serial_tx = Arc::new(Mutex::new(serial_tx));
            let (updater_tx, updater_rx): (Sender<Vec<_>>, _) = channel();

            uart_update::spawn(updater_rx, serial_tx);

            let (uart_tx, uart_rx) = uart.split();
            let mut uart_tx = Arc::new(Mutex::new(uart_tx));
            let uart_rx = Arc::new(Mutex::new(uart_rx));
            const BUF_SIZE: usize = 1024;
            let mut buf = [0u8; BUF_SIZE];
            loop {
                // Wait until there are bytes available to read
                let count: usize = match uart_rx.lock().unwrap().count() {
                    Ok(count) => count as usize,
                    Err(e) => {
                        println!("Error occured: {:?}", e);
                        0
                    }
                };

                if count == 0 {
                    if let Err(e) = handle_uart_tx(&mut uart_tx, &serial_rx) {}
                    thread::yield_now();
                    continue;
                }
                println!("Count: {}", count);

                let read_result = uart_rx
                    .lock()
                    .unwrap()
                    .read(&mut buf[..count], delay::NON_BLOCK);
                match read_result {
                    Ok(len) => {
                        let copied = Vec::from(&buf[..len as usize]);
                        updater_tx.send(copied).unwrap();
                    }
                    Err(e) => {
                        println!("Error during read: {:?}", e);
                        continue;
                    }
                };
            }
            fn handle_uart_tx(
                uart_tx: &mut Arc<Mutex<UartTxDriver>>,
                serial_rx: &Arc<Mutex<Receiver<Message<MessageTypeMcu>>>>,
            ) -> Result<(), Error> {
                let msg = match serial_rx.lock().unwrap().try_recv() {
                    Ok(v) => v,
                    Err(e) => return Err(e.into()),
                };

                let data = msg.serialize()?;
                match uart_tx.lock().unwrap().write(data.as_slice()) {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        println!("Failed to write: {:?}", e);
                        Err(e.into())
                    }
                }
            }
        })?;
    loop {
        thread::sleep(Duration::from_secs(10));
        println!("ping");
    }
}
