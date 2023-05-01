#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(alloc_error_handler)]

extern crate alloc;

use core::alloc::Layout;
use core::mem::MaybeUninit;
use alloc::format;
use embassy_embedded_hal::adapter::BlockingAsync;
use embassy_executor::_export::StaticCell;
use embassy_futures::select::select3;
use embassy_nrf::Peripheral;
use embassy_nrf::Peripherals;
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::Priority;
use embassy_nrf::pac::TWIS0;
use embassy_nrf::peripherals::P0_11;
use embassy_nrf::peripherals::P0_12;
use embassy_nrf::peripherals::TWISPI0;
use embassy_nrf::twim;
use embassy_nrf::twim::Twim;
use nrf_softdevice::ble;
use nrf_softdevice::ble::Connection;
use nrf_softdevice::ble::TxPower;
use nrf_softdevice::ble::peripheral;
use nrf_softdevice::{raw, Softdevice};
use nrf_softdevice::ble::{central, gatt_client, Address, AddressType};

use alloc_cortex_m::CortexMHeap;
use embassy_executor::Spawner;

use embassy_futures::join;
use swb_shared::Program;
use swb_shared::{Instruction, StyleVar};

use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_futures::select::select;
use embassy_futures::join::{join, join3, join4};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embedded_graphics::geometry::{Point, Size};
use toekomst::display::disp;

use toekomst::label::{label_once, label_once_bold, label_with};
use toekomst::{label};
use toekomst::display::request_redraw;
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
const HEAP_SIZE: usize = 32000;

static BINARY: &'static [u8] = include_bytes!("../ab.swb");

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

enum Scroll {
    Up,
    Down,
}

async fn scroll(notify: &Notify<Scroll>) {
    let up_fut = async {
        loop {
            toekomst::key::wait(Key::i).await;
            notify.notify(Scroll::Up);
        }
    };

    let down_fut = async {
        loop {
            toekomst::key::wait(Key::k).await;
            notify.notify(Scroll::Down);
        }
    };

    join(up_fut, down_fut).await;
}

static LINES_PER_SCROLL: i32 = 5;

async fn render_page(page: &Program, scroll_notify: &Notify<Scroll>) {
    let mut start_line = 0;
    let spacing = 2;
    let line_height = label::FONT.character_size.height + spacing;
    loop {
        let mut v = Vertical::new(Point::new(5, 2), spacing);

        let mut bold = StyleVarStack::new();
        let mut cur_line = 0;
        let mut line_space_left = 240 / line_height;
        for instr in &page.code {
            if line_space_left == 0 { break; }
            match instr {
                Instruction::Text(address) => {
                    cur_line += 1;
                    if cur_line < start_line + 1 { continue; }
                    //info!("text: {}", format!("{}", address).as_str());
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
                    line_space_left -= 1;
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
                    cur_line += 1;
                    if cur_line < start_line { continue; }
                    line_space_left -= 1;
                    v.push(label::FONT.character_size);
                }
                Instruction::Stop => {
                    break;
                }
            }
        }
        
        request_redraw();

        // We rendered our current version of the page, now wait for a scroll command
        let scroll = scroll_notify.wait().await;
        match scroll {
            Scroll::Up => {
                start_line -= LINES_PER_SCROLL;
                start_line = start_line.max(0);
                info!("Scrolling up to {}", start_line);
            },
            Scroll::Down => {
                start_line += LINES_PER_SCROLL;
                info!("Scrolling down to {}", start_line);
            }
        }

        let mut dp = disp().await;
        dp.clear();
    }
}

async fn ui(page: &Program) {
    let scroll_notify = Notify::new();
    let scroll_fut = scroll(&scroll_notify);
    join(scroll_fut, render_page(page, &scroll_notify)).await;
}

fn parse_key_state(value: u8) -> Option<Key> {
    if value >= 'a' as u8 && value <= 'z' as u8 {
        // SAFETY: We just verified this key is a valid character
        unsafe { Some(Key::from_u8(value - 'a' as u8)) }
    } else {
        None
    }
}

#[embassy_executor::task]
async fn keyboard_driver(twim: TWISPI0, p0_12: P0_12, p0_11: P0_11) {
    static I2C_BUS: StaticCell<Mutex::<ThreadModeRawMutex, Twim<TWISPI0>>> = StaticCell::new();
    let config = twim::Config::default();
    let irq = interrupt::take!(SPIM0_SPIS0_TWIM0_TWIS0_SPI0_TWI0);
    info!("Got irq");
    let i2c = Twim::new(twim, irq, p0_12, p0_11, config);
    let i2c_bus = Mutex::<ThreadModeRawMutex, _>::new(i2c);
    let i2c_bus = I2C_BUS.init(i2c_bus); 
    info!("Created I2C bus");
    let mut kb = Bbq10Kbd::new(I2cDevice::new(i2c_bus));
    info!("Initialized keyboard driver");
    loop {
        let key = kb.get_fifo_key_raw().await.unwrap();
        match key {
            KeyRaw::Pressed(value) => {
                match parse_key_state(value) {
                    Some(key) => toekomst::key::press_key(key),
                    None => {}
                };
            },
            _ => {}
        }
    }
}

#[embassy_executor::task]
async fn softdevice_driver(sd: &'static Softdevice) -> ! {
    sd.run().await
}

fn initialize_softdevice(spawner: &Spawner) -> &'static Softdevice {
    let config = nrf_softdevice::Config {
        clock: Some(raw::nrf_clock_lf_cfg_t {
            source: raw::NRF_CLOCK_LF_SRC_RC as u8,
            rc_ctiv: 16,
            rc_temp_ctiv: 2,
            accuracy: raw::NRF_CLOCK_LF_ACCURACY_500_PPM as u8,
        }),
        conn_gap: Some(raw::ble_gap_conn_cfg_t {
            conn_count: 6,
            event_length: 6,
        }),
        conn_gatt: Some(raw::ble_gatt_conn_cfg_t { att_mtu: 128 }),
        gatts_attr_tab_size: Some(raw::ble_gatts_cfg_attr_tab_size_t { attr_tab_size: 32768 }),
        gap_role_count: Some(raw::ble_gap_cfg_role_count_t {
            adv_set_count: 1,
            periph_role_count: 3,
            central_role_count: 3,
            central_sec_count: 0,
            _bitfield_1: raw::ble_gap_cfg_role_count_t::new_bitfield_1(0),
        }),
        gap_device_name: Some(raw::ble_gap_cfg_device_name_t {
            p_value: b"HelloRust" as *const u8 as _,
            current_len: 9,
            max_len: 9,
            write_perm: unsafe { core::mem::zeroed() },
            _bitfield_1: raw::ble_gap_cfg_device_name_t::new_bitfield_1(raw::BLE_GATTS_VLOC_STACK as u8),
        }),
        ..Default::default()
    };

    let sd = Softdevice::enable(&config);
    spawner.spawn(softdevice_driver(sd)).unwrap();
    sd
}

// Note: reversed!
const SERVER_ADDR: [u8; 6] = [0x26, 0x28, 0xec, 0xcf, 0x7f, 0x28];

#[nrf_softdevice::gatt_client(uuid = "feed")]
struct TestServiceClient {
    #[characteristic(uuid = "f00d", write, read, notify)]
    value: u8,
}

async fn scan_for_server(sd: &'static Softdevice) -> (TestServiceClient, Connection) {
    let addrs = &[&Address::new(
        AddressType::Public,
        SERVER_ADDR,
    )];
    let mut config = central::ConnectConfig::default();
    config.scan_config.whitelist = Some(addrs);
    let conn = central::connect(sd, &config).await.unwrap();
    info!("connected");

    (gatt_client::discover(&conn).await.unwrap(), conn)
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    {
        static mut HEAP: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        unsafe { ALLOCATOR.init(HEAP.as_ptr() as usize, HEAP_SIZE) }
    }

    let mut config = embassy_nrf::config::Config::default();
    config.time_interrupt_priority = Priority::P2;
    config.gpiote_interrupt_priority = Priority::P2;
    let p = embassy_nrf::init(config);

    let sd = initialize_softdevice(&spawner);
    info!("Initialized softdevice");
    info!("My address: {:?}", ble::get_address(sd));

    let (client, _) = scan_for_server(sd).await;
    client.value_write(&5).await.unwrap();

    spawner.spawn(keyboard_driver(p.TWISPI0, p.P0_12, p.P0_11)).unwrap();

    let program = parse_swb(BINARY);
    toekomst::display::init_disp(p.SPI2, p.P0_14, p.P0_13, p.P0_03, p.P0_02, Size::new(400, 240));
    info!("Display initialized");

    select(toekomst::display::run_disp(), ui(&program)).await;
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