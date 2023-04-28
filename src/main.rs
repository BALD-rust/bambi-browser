#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(alloc_error_handler)]

extern crate alloc;

use alloc::vec::Vec;
use core::alloc::Layout;
use core::mem::MaybeUninit;

use alloc_cortex_m::CortexMHeap;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_time::{Duration, Timer};

use swb_shared::Program;

#[allow(unused_imports)]
#[cfg(feature = "defmt")]
use {defmt_rtt as _, panic_probe as _};

pub(crate) mod fmt;
#[cfg(feature = "log")]
mod logger;

#[global_allocator]
static ALLOCATOR: CortexMHeap = CortexMHeap::empty();
const HEAP_SIZE: usize = 1024;

static BINARY: &'static [u8; 91] = include_bytes!("../output.swb");

fn parse_swb(bytes: &[u8]) -> Program {
    let result = Program::try_from(bytes);
    unwrap!(result)
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Initialize allocator
    {
        static mut HEAP: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        unsafe { ALLOCATOR.init(HEAP.as_ptr() as usize, HEAP_SIZE) }
    }

    let p = embassy_nrf::init(Default::default());
    #[cfg(feature = "log")]
    logger::init(&_spawner, p.USBD);

    let program = parse_swb(BINARY);
    info!("{}", program);
}

#[cfg(feature = "defmt")]
#[defmt::panic_handler]
fn panic() -> ! {
    cortex_m::asm::udf();
}

#[alloc_error_handler]
fn oom(_: Layout) -> ! {
    cortex_m::asm::udf();
}

#[cfg(not(feature = "defmt"))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    cortex_m::asm::udf();
}