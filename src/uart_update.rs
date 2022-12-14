use std::{
    sync::{
        mpsc::{Receiver, Sender},
        Arc, Mutex,
    },
    thread,
};

use embedded_svc::io::Write;
use esp_idf_svc::ota::EspOta;
use esp_idf_sys::esp_restart;
use messages::{Message, MessageTypeHost, MessageTypeMcu, UpdateStatus};
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
pub fn spawn(rx: Receiver<Vec<u8>>, tx: Arc<Mutex<Sender<Message<MessageTypeMcu>>>>) {
    let builder = thread::Builder::new()
        .name("uart_update thread".to_string())
        .stack_size(100 * 1024);
    let _handler = builder.spawn(move || {
        let mut sm = StateMachine::new(Context);

        let mut ota = match EspOta::new() {
            Ok(v) => {
                println!("Constructed ota object");
                v
            }
            Err(e) => {
                println!("Could not construct ota object... {:?}", e);
                return e;
            }
        };
        let mut ota_update = None;

        let mut expected_seg_id = 0;
        loop {
            println!("Running loop on uart_update");
            if let Ok(data) = rx.recv() {
                // Deserialize message from UART
                let msg = match Message::<MessageTypeHost>::deserialize(&data[..]) {
                    Ok(msg) => msg.payload.message_type,
                    Err(e) => {
                        println!("Error occured in deserialize: {:?}", e);
                        continue;
                    }
                };
                match msg {
                    MessageTypeHost::UpdateStart if sm.state == States::Init => {
                        println!("Starting update");

                        println!(
                            "Current slot: {:?}",
                            match ota.get_running_slot() {
                                Ok(val) => val.label.to_string(),
                                Err(e) => e.to_string(),
                            }
                        );
                        println!("Updating slot: {:?}", ota.get_update_slot().unwrap().label);

                        ota_update = Some(ota.initiate_update().unwrap());
                        tx.lock()
                            .unwrap()
                            .send(Message::new(MessageTypeMcu::UpdateStartStatus(
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
                            tx.lock()
                                .unwrap()
                                .send(Message::new(MessageTypeMcu::UpdateSegmentStatus(
                                    UpdateStatus::Retry(Some(expected_seg_id as u16)),
                                )))
                                .unwrap();
                            continue;
                        }

                        let ota_update = ota_update.as_mut().unwrap();
                        match ota_update.write(segment) {
                            Ok(_) => (),
                            Err(e) => {
                                println!("Received invalid segment: {:?} ({:?})", segment, e);
                                continue;
                            }
                        }

                        expected_seg_id = id + 1;
                        tx.lock()
                            .unwrap()
                            .send(Message::new(MessageTypeMcu::UpdateSegmentStatus(
                                UpdateStatus::Ok,
                            )))
                            .unwrap();

                        sm.process_event(Events::SegmentOk).unwrap();
                    }
                    MessageTypeHost::UpdateEnd if sm.state == States::WaitingForData => {
                        let ota_update = ota_update.take().unwrap();
                        ota_update.complete().unwrap();

                        println!("Restarting the system!");
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
                    _ => println!("Invalid state"),
                };
            }
        }
    });
}
