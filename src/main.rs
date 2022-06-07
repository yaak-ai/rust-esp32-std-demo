#![allow(unused_imports)]
#![allow(clippy::single_component_path_imports)]

use std::any::Any;
use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::sync::{
    mpsc::{channel, Receiver, Sender},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::*;

use embedded_svc::mutex::Mutex;
use esp_idf_hal::serial::isr_config::IsrConfig;
use esp_idf_sys::c_types::c_void;
use log::*;

use embedded_hal::delay::blocking::DelayUs;
use embedded_hal::nb::block;
use embedded_hal::serial::nb::{Read, Write};

use embedded_svc::ota::*;
use embedded_svc::sys_time::SystemTime;
use embedded_svc::timer::TimerService;
use embedded_svc::timer::*;

use esp_idf_svc::ota;
use esp_idf_svc::systime::EspSystemTime;
use esp_idf_svc::timer::*;

use esp_idf_hal::adc;
use esp_idf_hal::delay;
use esp_idf_hal::gpio;
use esp_idf_hal::i2c;
use esp_idf_hal::prelude::*;
use esp_idf_hal::serial;
use esp_idf_hal::spi;

use esp_idf_sys::*;
use esp_idf_sys::{esp, EspError};

use anyhow::{anyhow, bail, Result};

use messages::*;

mod uart_update;

// Use UART1 for updater
type Uart = serial::UART1;

fn main() -> Result<()> {
    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!(
        "Current partition: {:?}",
        ota::EspOta::new()
            .unwrap()
            .get_running_slot()
            .unwrap()
            .get_label()
    );

    let peripherals = Peripherals::take().unwrap();
    let pins = peripherals.pins;

    // Prepare serial port
    let mut serial: serial::Serial<Uart, _, _> = serial::Serial::new(
        peripherals.uart1,
        serial::Pins {
            tx: pins.gpio17,
            rx: pins.gpio16,
            cts: None,
            rts: None,
        },
        serial::config::Config {
            baudrate: Hertz(921_600),
            event_queue_size: 10,
            rx_buffer_size: serial::UART_FIFO_SIZE * 20,
            ..Default::default()
        },
    )?;
    serial.listen_rx()?;

    // Setup channels for the UART RX/TX tasks to communicate with the updater task
    let (serial_tx, serial_rx): (Sender<Message<MessageTypeMcu>>, _) = channel();
    let (updater_tx, updater_rx): (Sender<Vec<_>>, _) = channel();
    // Start the updater task
    uart_update::spawn(updater_rx, serial_tx);

    // Start the task for receiving data from UART
    let (mut uart_tx, mut uart_rx, event_handle) = serial.split();
    let event_handle = event_handle.unwrap();
    thread::Builder::new()
        .stack_size(25 * 1024)
        .spawn(move || {
            const BUF_SIZE: usize = 1024;
            let mut buf = [0u8; BUF_SIZE];
            loop {
                // Wait for RX event
                let event = match event_handle.wait_for_event() {
                    None => continue,
                    Some(event) => event,
                };

                match event.get_type() {
                    // Pass any data directly to the updater task that will handle message decoding
                    serial::Event::Data => match uart_rx.read_bytes(&mut buf) {
                        Ok(len) => {
                            let copied = Vec::from(&buf[..len as usize]);
                            updater_tx.send(copied).unwrap();
                        }
                        Err(_) => continue,
                    },
                    serial::Event::FifoOvf => log::info!("Woops, RX buffer overflowed"),
                    e => log::info!("Unexpected event: {:?}", e),
                };
            }
        })?;

    // Start a task that handles sending answers from the updater over UART
    thread::Builder::new().stack_size(2 * 1024).spawn(move || {
        loop {
            if let Err(e) = handle_uart_tx(&mut uart_tx, &serial_rx) {
                log::error!("{}", e);
            }
        }

        fn handle_uart_tx(
            uart_tx: &mut serial::Tx<Uart>,
            serial_rx: &Receiver<Message<MessageTypeMcu>>,
        ) -> Result<(), anyhow::Error> {
            let msg = serial_rx.recv()?;

            let data = msg.serialize()?;
            match uart_tx.write_bytes(data.as_slice()) {
                Ok(_) => Ok(()),
                Err(_) => bail!("Failed to write"),
            }
        }
    })?;

    loop {
        thread::sleep(Duration::from_secs(10));
        log::info!("ping");
    }
}
