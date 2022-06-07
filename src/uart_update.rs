use std::{
    borrow::Borrow,
    ffi::CStr,
    ptr,
    sync::mpsc::{Receiver, Sender},
    thread,
};

use embedded_svc::{
    io::Write,
    ota::{Ota, OtaSlot, OtaUpdate},
};
use messages::{Message, MessageTypeHost, MessageTypeMcu, UpdateStatus};
use esp_idf_svc::ota::EspOta;
use esp_idf_sys::{c_types::c_void, *};
use smlang::statemachine;

// Updater statemachine
statemachine! {
    transitions: {
        *Init + UpdateStart = WaitingForData,
        WaitingForData + SegmentOk = WaitingForData,

        Init | WaitingForData + Cancel = Init,
    }
}
pub struct Context;
impl StateMachineContext for Context {}

/// Spawn a new task that will handles raw messages from UART
pub fn spawn(rx: Receiver<Vec<u8>>, tx: Sender<Message<MessageTypeMcu>>) {
    thread::spawn(move || {
        let mut sm = StateMachine::new(Context);

        let mut ota = EspOta::new().unwrap();
        let mut ota_update = None;

        let mut expected_seg_id = 0;
        loop {
            if let Ok(data) = rx.recv() {
                // Deserialize message from UART
                let msg = match Message::<MessageTypeHost>::deserialize(&data[..]) {
                    Ok(msg) => msg.payload.message_type,
                    Err(e) => {
                        log::error!("{:?}", e);
                        continue;
                    }
                };
                match msg {
                    MessageTypeHost::UpdateStart if sm.state == States::Init => {
                        log::info!("Starting update");

                        log::info!(
                            "Current slot: {:?}",
                            ota.get_running_slot().unwrap().get_label().unwrap()
                        );
                        log::info!(
                            "Updating slot: {:?}",
                            ota.get_update_slot().unwrap().get_label().unwrap()
                        );

                        ota_update = Some(ota.initiate_update().unwrap());
                        tx.send(Message::new(MessageTypeMcu::UpdateStartStatus(
                            UpdateStatus::Ok,
                        )))
                        .unwrap();
                        expected_seg_id = 0;

                        sm.process_event(Events::UpdateStart).unwrap();
                    }
                    MessageTypeHost::UpdateSegment(id, segment)
                        if sm.state == States::WaitingForData =>
                    {
                        if expected_seg_id != id {
                            tx.send(Message::new(MessageTypeMcu::UpdateSegmentStatus(
                                UpdateStatus::Retry(Some(expected_seg_id as u16)),
                            )))
                            .unwrap();
                            continue;
                        }

                        let ota_update = ota_update.as_mut().unwrap();
                        ota_update.do_write(segment).unwrap();

                        expected_seg_id = id + 1;
                        tx.send(Message::new(MessageTypeMcu::UpdateSegmentStatus(
                            UpdateStatus::Ok,
                        )))
                        .unwrap();

                        sm.process_event(Events::SegmentOk).unwrap();
                    }
                    MessageTypeHost::UpdateEnd if sm.state == States::WaitingForData => {
                        let ota_update = ota_update.take().unwrap();
                        ota_update.complete().unwrap();

                        log::info!("Restarting the system!");
                        unsafe { esp_restart() };
                    }
                    MessageTypeHost::Cancel => match sm.state {
                        States::Init => (),
                        States::WaitingForData => {
                            let ota_update = ota_update.take().unwrap();
                            ota_update.abort().unwrap();
                            sm.process_event(Events::Cancel).unwrap();
                        }
                    },
                    _ => log::error!("Invalid state"),
                };
            }
        }
    });
}
