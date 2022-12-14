mod simple_ota;
mod uart_update;
use crate::simple_ota::serial_ota;
use crate::simple_ota::Error;

fn main() -> Result<(), Error> {
    esp_idf_sys::link_patches();
    match serial_ota() {
        Ok(v) => v,
        Err(e) => {
            println!("Error occured during update: {:?}", e);
            return Err(e);
        }
    };

    Ok(())
}
