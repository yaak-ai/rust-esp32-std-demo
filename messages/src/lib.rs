use std::fmt::Display;

use anyhow::{Result, anyhow};
use postcard::{to_allocvec, from_bytes};
use serde::{Serialize, Deserialize};
use crc::{Crc, CRC_16_IBM_3740 as CRC_ALG}; // Also called CRC-16-CCITT-FALSE
extern crate alloc;

pub const VERSION: u8 = 1;
pub const CRC: Crc<u16> = Crc::<u16>::new(&CRC_ALG);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    ChecksumError,
}
impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum UpdateStatus {
    Ok,
    Retry(Option<u16>),
    Failed,
}


/// Message sent from the MCU to the host
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum MessageTypeMcu {
    /// Send an ADC measurement
    Adc(u32),
    /// Send status if ready or not to receive a software update
    /// Follows the reception of `MessageTypeHost::UpdateStart`
    UpdateStartStatus(UpdateStatus),
    /// Send last segment status
    /// Follows the reception of `MessageTypeHost::UpdateSegment`
    UpdateSegmentStatus(UpdateStatus),
    /// Send final status of the update
    /// Follows the reception of `MessageTypeHost::UpdateEnd`
    UpdateEndStatus(UpdateStatus),
}

/// Message sent from the host to the MCU
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub enum MessageTypeHost<'a> {
    /// Ask the MCU to be ready to receive an update
    UpdateStart,
    /// Send an update segment
    UpdateSegment(u16, &'a [u8]),
    /// Finish the update process
    UpdateEnd,
    /// Cancel any current operation
    Cancel,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub struct MessagePayload<T> {
    version: u8,
    pub message_type: T,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy)]
pub struct Message<T> {
    pub payload: MessagePayload<T>,
    checksum: u16,
}
impl<'de, T: Serialize + Deserialize<'de>> Message<T> {
    /// Create a new message from a `message_type` and compute its CRC
    pub fn new(message_type: T) -> Message<T> {
        let payload = MessagePayload::<T> { version: VERSION, message_type };
        // TODO: this is very bad
        let payload_bytes = to_allocvec(&payload).unwrap();
        let crc = CRC.checksum(&payload_bytes);

        Message { 
            payload,
            checksum: crc
        }
    }

    /// Serialize the message to a vector of bytes
    pub fn serialize(&self) -> Result<Vec<u8>> {
        to_allocvec(&self).map_err(|e| anyhow!(e))
    }

    /// Deserialize a vector of bytes into a message
    pub fn deserialize(bytes: &'de [u8]) -> Result<Message<T>> {
        let res: Result<Message<T>> = from_bytes(bytes).map_err(|e| anyhow!(e));
        match &res {
            Ok(msg) if !msg.is_crc_valid() => Err(anyhow!(Error::ChecksumError)),
            _ => res,
        }
    }

    /// Check if the CRC is valid
    pub fn is_crc_valid(&self) -> bool {
        // TODO: this is very bad
        let payload_bytes = to_allocvec(&self.payload).unwrap();
        let crc = CRC.checksum(&payload_bytes);

        self.checksum == crc
    }
}

// trait SerDer<'de>: Serialize + Deserialize<'de> {
//     /// Serialize the message to a vector of bytes
//     fn serialize(&self) -> Result<Vec<u8>> {
//         to_allocvec(&self).map_err(|e| anyhow!(e))
//     }

//     /// Deserialize a vector of bytes into a message
//     fn deserialize(bytes: &'de [u8]) -> Result<Self> {
//         from_bytes(bytes).map_err(|e| anyhow!(e))
//         // let res: Result<Self> = from_bytes(bytes).map_err(|e| anyhow!(e));
//         // match &res {
//         //     Ok(msg) if !msg.is_crc_valid() => Err(anyhow!(Error::ChecksumError)),
//         //     _ => res,
//         // }
//     }
// }


#[cfg(test)]
mod tests {
    mod mcu {
        use crate::*;

        #[test]
        fn adc() {
            let raw = [VERSION, 0x00, 0xB7, 0x26, 0xCA, 0x62];
            let msg = Message::new(MessageTypeMcu::Adc(0x1337));
            let msg_bytes = Message::serialize(&msg).unwrap();
            assert_eq!(msg_bytes, raw);

            let des_msg: Message<MessageTypeMcu> = Message::deserialize(&msg_bytes).unwrap();
            assert_eq!(msg, des_msg);

            let msg_from_raw: Message<MessageTypeMcu> = from_bytes(&raw).unwrap();
            assert_eq!(msg, msg_from_raw);
        }

        #[test]
        fn update_start_failed() {
            let raw = [VERSION, 0x01, 0x02, 223, 209, 3];
            let msg = Message::new(MessageTypeMcu::UpdateStartStatus(UpdateStatus::Failed));
            let msg_bytes = Message::serialize(&msg).unwrap();
            assert_eq!(msg_bytes, raw);

            let des_msg: Message<MessageTypeMcu> = Message::deserialize(&msg_bytes).unwrap();
            assert_eq!(msg, des_msg);

            let msg_from_raw: Message<MessageTypeMcu> = from_bytes(&raw).unwrap();
            assert_eq!(msg, msg_from_raw);
        }

        #[test]
        fn update_retry_id() {
            let raw = [VERSION, 0x02, 0x01, 0x01, 154, 5, 178, 210, 3];
            let msg = Message::new(MessageTypeMcu::UpdateSegmentStatus(UpdateStatus::Retry(Some(666))));
            let msg_bytes = Message::serialize(&msg).unwrap();
            assert_eq!(msg_bytes, raw);

            let des_msg: Message<MessageTypeMcu> = Message::deserialize(&msg_bytes).unwrap();
            assert_eq!(msg, des_msg);

            let msg_from_raw: Message<MessageTypeMcu> = from_bytes(&raw).unwrap();
            assert_eq!(msg, msg_from_raw);
        }

        #[test]
        fn bad_checksum() {
            let raw = [VERSION, 0x00, 0xB7, 0x26, 0x00, 0x00];
            let res_des = Message::<MessageTypeMcu>::deserialize(&raw);
            assert!(res_des.is_err());
            assert_eq!((res_des.unwrap_err().downcast::<Error>().unwrap()), Error::ChecksumError);
        }
    }

    mod host {
        use crate::*;

        #[test]
        fn update_start() {
            let raw = [1, 0, 190, 92];
            let msg = Message::new(MessageTypeHost::UpdateStart);
            let msg_bytes = msg.serialize().unwrap();
            println!("{:?}", msg_bytes);
            assert_eq!(msg_bytes, raw);

        }
        
        #[test]
        fn cancel() {
            let raw = [VERSION, 0x03, 0xDD, 0x3C];
            let msg = Message::new(MessageTypeHost::Cancel);
            let msg_bytes = msg.serialize().unwrap();
            assert_eq!(msg_bytes, raw);

            let des_msg: Message<MessageTypeHost> = Message::deserialize(&msg_bytes).unwrap();
            assert_eq!(msg, des_msg);

            let msg_from_raw: Message<MessageTypeHost> = from_bytes(&raw).unwrap();
            assert_eq!(msg, msg_from_raw);
        }

        #[test]
        fn update_segment() {
            let raw = [VERSION, 0x01, 0x9A, 0x05, 4, 1, 2, 3, 0xFF, 0xBE, 0x84, 0x01];
            let msg = Message::new(MessageTypeHost::UpdateSegment(666, &[1, 2, 3, 0xFF]));
            let msg_bytes = msg.serialize().unwrap();
            assert_eq!(msg_bytes, raw);

            let des_msg: Message<MessageTypeHost> = Message::deserialize(&msg_bytes).unwrap();
            assert_eq!(msg, des_msg);

            let msg_from_raw: Message<MessageTypeHost> = from_bytes(&raw).unwrap();
            assert_eq!(msg, msg_from_raw);

            match des_msg.payload.message_type {
                MessageTypeHost::UpdateSegment(cnt, bytes) => {
                    assert_eq!(cnt, 666);
                    assert_eq!(bytes, [1, 2, 3, 0xFF]);
                },
                _ => assert!(false)
            }
        }
    }
}
