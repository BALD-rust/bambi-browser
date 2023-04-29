#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::alloc::Layout;
use core::mem::MaybeUninit;
use alloc::format;
use alloc::string::ToString;
use alloc::string::String;
use embassy_executor::_export::StaticCell;
use embassy_nrf::interrupt;
use embassy_nrf::peripherals::TWISPI0;
use embassy_nrf::twim;
use embassy_nrf::twim::Twim;

use core::future::poll_fn;
use core::task::Poll;

use alloc_cortex_m::CortexMHeap;
use embassy_executor::Spawner;

use embassy_futures::join;
use swb_shared::Program;
use swb_shared::{Instruction, StyleVar};

use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_futures::select::select;
use embassy_futures::join::{join, join3, join4};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embedded_graphics::geometry::{Point, Size};
use toekomst::display::disp;

use toekomst::label::{label_once, label_once_bold, label_with};
use toekomst::{label, request_redraw};
use toekomst::layout::Vertical;
use toekomst::notify::Notify;
use toekomst::input::Input;
use toekomst::key::Accel;
use toekomst::key::Key;
use toekomst::widget::Widget;
use toekomst::button::Button;
use bbq10kbd::{Bbq10Kbd, KeyStatus, KeyRaw};

#[allow(unused_imports)]
#[cfg(feature = "defmt")]
use {defmt_rtt as _, panic_probe as _};

pub(crate) mod fmt;
#[cfg(feature = "log")]
mod logger;

#[global_allocator]
static ALLOCATOR: CortexMHeap = CortexMHeap::empty();
const HEAP_SIZE: usize = 64000;

static BINARY: &'static [u8] = include_bytes!("../rust_datatypes.swb");

fn parse_swb(bytes: &[u8]) -> Program {
    let result = Program::try_from(bytes);
    match result {
        Ok(program) => program,
        Err(swb_shared::Error(e)) => {
            error!("error parsing: {}", e);
            panic!("fatal error");
        }
    }
}

struct StyleVarStack {
    state: u32,
}

impl StyleVarStack {
    pub fn new() -> Self {
        Self {
            state: 0,
        }
    }
    
    pub fn push(&mut self) {
        self.state += 1;
    }

    pub fn pop(&mut self) {
        self.state -= 1;
    }

    pub fn is_enabled(&self) -> bool {
        self.state > 0
    }
}

#[derive(Copy, Clone)]
enum Cmd {
    Up,
    Down
}

async fn render_page(page: &Program) {
    loop {
        let mut v = Vertical::new(Point::new(5, 10), 2);

        let mut bold = StyleVarStack::new();
        for instr in &page.code {
            break;
            match instr {
                Instruction::Text(address) => {
                    let str = 
                        page
                            .text
                            .as_bytes()
                            .get(
                                address.base.0 as usize..
                                (address.base.offset(address.range as i32).0 as usize))
                            .unwrap();
                    let str = core::str::from_utf8(str).unwrap();
                    if bold.is_enabled() {
                        label_once_bold(str, v.push(label::FONT.character_size)).await;
                    } else {
                        label_once(str, v.push(label::FONT.character_size)).await;
                    }
                }
                Instruction::Push(StyleVar::Bold) => {
                    bold.push();
                }
                Instruction::Push(_) => {}
                Instruction::Pop(StyleVar::Bold) => {
                    bold.pop();
                }
                Instruction::Pop(_) => {}
                Instruction::Endl => {
                    v.push(label::FONT.character_size);
                }
                Instruction::Stop => {
                    break;
                }
            }
        }

        //let mut dp = disp().await;
        //dp.flush_buffer();
        //request_redraw();
    }
}

async fn ui(page: &Program) {
    let ac = Accel::new::<u8>();
    let cmd_notif = Notify::new();
    toekomst::key::wait(Key::a).await;
    info!("a pressed");
    let (btn_plus, ac) = 
        Button::new(ac, Point::new(5, 0), "Down", &cmd_notif, Cmd::Down);
    let text_notif = Notify::new_preoccupied("Count: 0".to_string());
    let count_label = label_with(&text_notif, Point::new(5, 20));
    let cmd_fut = async {
        loop {
            match cmd_notif.wait().await {
                Cmd::Up => info!("up"),
                Cmd::Down => info!("down"),
            };
            text_notif.notify(format!("message"));
            let mut dp = disp().await;
            dp.flush_buffer();
            request_redraw();
        }
    };

    let render_fut = render_page(page);
    join3(cmd_fut, btn_plus.render(), count_label).await;
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

    static I2C_BUS: StaticCell<Mutex::<ThreadModeRawMutex, Twim<TWISPI0>>> = StaticCell::new();
    let config = twim::Config::default();
    let irq = interrupt::take!(SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
    let i2c = Twim::new(p.TWISPI0, irq, p.P0_03, p.P0_04, config);
    let i2c_bus = Mutex::<ThreadModeRawMutex, _>::new(i2c);
    let i2c_bus = I2C_BUS.init(i2c_bus); 
    let kb = Bbq10Kbd::new(I2cDevice::new(i2c_bus));
    

    let program = parse_swb(BINARY);

    toekomst::display::init_disp(p.SPI2, p.P0_14, p.P0_13, p.P0_03, p.P0_02, Size::new(400, 240));
    info!("Display initialized");

    select(toekomst::display::run_disp(), ui(&program)).await;
    //select(toekomst::display::run_disp(), chat_ui()).await;
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
