use anyhow::Context;
use argh::FromArgs;
use messages::{Message, MessageTypeHost, MessageTypeMcu, UpdateStatus};
use serialport::{self, SerialPort, TTYPort};
use thiserror::Error;
use std::{io::{stdin, stdout, Write, Read}, path::PathBuf, fs::File, time::Duration};

#[derive(Debug, Error)]
enum Error {
    #[error("No serial port found")]
    NoSerialPort,
    #[error("Binary does not exists at {0}")]
    BinDoesntExist(String),
    #[error("Writing to UART failed")]
    ComWriteFailed,
    #[error("Received invalid response from UART")]
    ComInvalidResponse,
    #[error("Received critical error response from UART")]
    ComCriticalError,
}

#[derive(FromArgs)]
/// Reach new heights.
struct UartArgs {
    /// path to the binary file to flash
    #[argh(positional)]
    bin_path: PathBuf,

    /// UART port to use
    #[argh(option)]
    uart: Option<String>,

    /// baudrate
    #[argh(option, default = "921_600")]
    baudrate: u32,
}

fn main() -> Result<(), anyhow::Error> {
    let args: UartArgs = argh::from_env();

    if !args.bin_path.exists() {
        Err(Error::BinDoesntExist(args.bin_path.display().to_string()))?
    }

    // Get the serial port name
    let uart_port = match args.uart {
        // Either from arguments
        Some(user_port) => user_port,
        // Or from user input
        None => {
            loop {
                let available_ports = serialport::available_ports().unwrap();
                if available_ports.is_empty() {
                    Err(Error::NoSerialPort)?
                }

                // Display available serial ports
                println!("Available ports: ");
                for (i, p) in available_ports.iter().enumerate() {
                    print!("{i}: {}", p.port_name);
                    match &p.port_type {
                        serialport::SerialPortType::UsbPort(u)  => match &u.product {
                            Some(product) => println!(" ({})", product),
                            None => println!(),
                        },
                        _ => println!(),
                    }
                }

                // Get user input
                let mut s = String::new();
                print!("Enter the index of the desired UART port: ");
                
                let _ = stdout().flush();
                s.clear();
                stdin().read_line(&mut s).expect("Did not enter a correct string");
                let i: usize = match s.trim().parse() {
                    Ok(i) => i,
                    Err(_) => {
                        println!("Expecting an index\n");
                        continue;
                    },
                };
                if i < available_ports.len() {
                    break available_ports.get(i).unwrap().port_name.clone();
                }
                println!("Index out-of-range\n");
            }
        }
    };

    // Prepare serial port
    let mut serial_port = serialport::new(&uart_port, args.baudrate)
        .timeout(Duration::from_millis(500)) // This seems to be useless for `read()`
        .open_native()
        .context(format!("opening {uart_port}"))?;

    // Open firmware
    let mut file = File::open(args.bin_path)?;
    let mut firmware = Vec::new();
    file.read_to_end(&mut firmware)?;

    // Cancel any previous operation
    let msg_buffer = Message::new(MessageTypeHost::Cancel).serialize()?;
    serial_port.write(msg_buffer.as_slice())?;

    // TODO: implement Cancel ACK on ESP instead of waiting
    std::thread::sleep(Duration::from_millis(50));

    // Start update
    let msg_buffer = Message::new(MessageTypeHost::UpdateStart).serialize()?;
    serial_port.write(msg_buffer.as_slice())?;

    std::thread::sleep(Duration::from_millis(50));

    // ACK start update
    let mut msg_buffer: Vec<u8> = vec![0; 6];
    serial_port.read_exact(msg_buffer.as_mut_slice()).context("reading start update ACK")?;
    let rx_msg = Message::<MessageTypeMcu>::deserialize(msg_buffer.as_slice())?;
    
    if !matches!(rx_msg.payload.message_type, MessageTypeMcu::UpdateStartStatus(UpdateStatus::Ok)) {
        Err(Error::ComInvalidResponse)?
    }

    // Split firmware into chunks to send to ESP
    let chunks: Vec<&[u8]> = firmware.chunks(110).collect();
    let mut i = 0;
    while i < chunks.len() {
        const MAX_RETRY: usize = 5;
        'retry: for retry_cnt in 0..MAX_RETRY+1 {
            if retry_cnt == MAX_RETRY {
                Err(Error::ComWriteFailed)?
            }

            println!("Sending chunk {i}");
            let chunk = chunks.get(i).unwrap();
            match send_chunk(&mut serial_port, i, chunk) {
                Ok(status) => match status {
                    UpdateStatus::Ok => break 'retry,
                    UpdateStatus::Retry(Some(id)) if (id as usize) <= i => {
                        println!("Retrying segment {}, {}/{}", id, retry_cnt+1, MAX_RETRY);
                        i = id as usize;
                        continue 'retry;
                    },
                    _ => Err(Error::ComCriticalError)?,
                },
                Err(e) => Err(e)?,
            }

            fn send_chunk(serial_port: &mut TTYPort, id: usize, chunk: &[u8]) -> Result<UpdateStatus, anyhow::Error> {
                let msg = Message::new(MessageTypeHost::UpdateSegment(id as u16, chunk)).serialize()?;
                serial_port.write(msg.as_slice())?;

                // There is no way of waiting for unknown length of data
                // so we wait for a few bytes, and check if there is more in the buffer afterwards
                // That's pretty sad.
                let mut msg_buffer: Vec<u8> = vec![0; 6];
                serial_port.read_exact(&mut msg_buffer[..6])?;
                match serial_port.bytes_to_read() {
                    Ok(bytes_to_read) if bytes_to_read > 0 => {
                        let mut tmp = vec![0; bytes_to_read as usize];
                        serial_port.read_exact(&mut tmp)?;
                        msg_buffer.extend(tmp);
                    },
                    _ => (),
                };
                let rx_msg = Message::<MessageTypeMcu>::deserialize(msg_buffer.as_mut_slice())
                    .context(format!("deserializing {:?}", msg_buffer.as_slice()))?;

                match rx_msg.payload.message_type {
                    MessageTypeMcu::UpdateSegmentStatus(status) => Ok(status),
                    _ => Err(Error::ComInvalidResponse)?
                }
            }
        }

        // Go to next chunk if nothing happened
        i += 1;
    }

    let msg = Message::new(MessageTypeHost::UpdateEnd).serialize()?;
    serial_port.write(msg.as_slice())?;


    Ok(())
}
