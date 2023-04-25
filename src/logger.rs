use embassy_executor::Spawner;
use embassy_nrf::interrupt;
use embassy_nrf::peripherals::USBD;
use embassy_nrf::usb::{Driver, PowerUsb};

#[embassy_executor::task]
async fn logger_task(driver: Driver<'static, USBD, PowerUsb>) {
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

pub fn init(spawner: &Spawner, usbd: USBD) {
    let irq = interrupt::take!(USBD);
    let power_irq = interrupt::take!(POWER_CLOCK);
    let driver = Driver::new(usbd, irq, PowerUsb::new(power_irq));
    spawner.spawn(logger_task(driver)).unwrap();
}