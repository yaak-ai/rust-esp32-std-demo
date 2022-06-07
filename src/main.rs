#![allow(unused_imports)]
#![allow(clippy::single_component_path_imports)]

use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::sync::Arc;
use std::{thread, time::*};
use std::fmt::Write;

use anyhow::{bail, Result};

use embedded_hal::nb::block;
use embedded_hal::serial::nb::Read;
use embedded_svc::mutex::Mutex;
use esp_idf_hal::serial::isr_config::IsrConfig;
use esp_idf_sys::c_types::c_void;
use log::*;

use embedded_hal::delay::blocking::DelayUs;

use embedded_svc::sys_time::SystemTime;
use embedded_svc::timer::TimerService;
use embedded_svc::timer::*;
use embedded_svc::ota::{Ota, OtaSlot, OtaUpdate};

use esp_idf_svc::ota::{EspOta};
use esp_idf_svc::systime::EspSystemTime;
use esp_idf_svc::timer::*;

use esp_idf_hal::adc;
use esp_idf_hal::delay;
use esp_idf_hal::gpio;
use esp_idf_hal::i2c;
use esp_idf_hal::prelude::*;
use esp_idf_hal::serial::{self, Event, Interrupt};
use esp_idf_hal::spi;

use esp_idf_sys::{*};
use esp_idf_sys::{esp, EspError};


fn main() -> Result<()> {

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().unwrap();
    let pins = peripherals.pins;

    let mut serial: serial::Serial<serial::UART1, _, _> = serial::Serial::new(
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
            rx_buffer_size: serial::UART_FIFO_SIZE * 10,
            ..Default::default()
        },
    )?;

    serial.listen_rx()?;
    thread::spawn(move || {
        loop {
            let event = match serial.wait_for_event() {
                None => continue,
                Some(e) => e
            };

            match event.get_type() {
                Event::Data => loop {
                    match serial.read() {
                        Ok(c) => {
                            match c as char {
                                '\r' => serial.write_str("\r\n").unwrap(),
                                _ => serial.write_char(c as char).unwrap(),
                            }
                        },
                        Err(_) => break,
                    }
                },
                Event::FifoOvf => log::info!("Woops, RX buffer overflowed"),
                e => log::info!("Unexpected Error: {:?}", e),
            };
        }
    });

    loop {
        thread::sleep(Duration::from_secs(1));
        log::info!("ping");
    }
}
