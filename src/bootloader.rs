use crate::{gpio, rpi, timer, uart, crc32};
use core::ptr;

// protocol values shared between the pi and unix side.

const ARMBASE: u32 = 0x8000; // where program gets linked.  we could send this.

// the weird numbers are to try to help with debugging
// when you drop a byte, flip them, corrupt one, etc.
const BOOT_START: u32 = 0xFFFF0000;

const PUT_PROG_INFO: u32 = 0x33334444; // unix sends
const PUT_CODE: u32 = 0x77778888; // unix sends

const GET_PROG_INFO: u32 = 0x11112222; // pi sends
const GET_CODE: u32 = 0x55556666; // pi sends
const BOOT_SUCCESS: u32 = 0x9999AAAA; // pi sends on success
const BOOT_ERROR: u32 = 0xBBBBCCCC; // pi sends on failure.
const PRINT_STRING: u32 = 0xDDDDEEEE; // pi sends to print a string.

// error codes from the pi to unix
const BAD_CODE_ADDR: u32 = 0xdeadbeef;
const BAD_CODE_CKSUM: u32 = 0xfeedface;

const BOOTLOADER_START: u32 = 0x00200000;

pub enum BOOTERROR {
    TIMEOUT,
    NO_CODE,
    BAD_ADDR,
    BAD_CKSUM,
    OTHER,
}

#[no_mangle]
pub fn boot_put32(val: u32) {
    unsafe {
        uart::write_byte(((val >> 0) & 0b11111111) as u32);
        uart::write_byte(((val >> 8) & 0b11111111) as u32);
        uart::write_byte(((val >> 16) & 0b11111111) as u32);
        uart::write_byte(((val >> 24) & 0b11111111) as u32);
    }
}

#[no_mangle]
pub fn boot_get32() -> u32 {
    unsafe {
        let mut val1 = uart::read_byte();
        let mut val2 = uart::read_byte();
        let mut val3 = uart::read_byte();
        let mut val4 = uart::read_byte();
        let mut ret =
            ((val4 as u32) << 24) | ((val3 as u32) << 16) | ((val2 as u32) << 8) | (val1 as u32);
        ret
    }
}

#[no_mangle]
// simple  protocol for sending strings from pi to unix
// [PRINT_STRING, len(msg), msg]
pub fn boot_putk(msg: &str) {
    boot_put32(PRINT_STRING);
    boot_put32(msg.len() as u32);
    for byte in msg.as_bytes() {
        unsafe {
            uart::write_byte(*byte as u32);
        }
    }
}

#[no_mangle]
// Spins checking for data until a timeout
pub fn has_data_timeout(timeout: u32) -> Result<(), BOOTERROR> {
    let start = timer::timer_get_usec();
    let mut curr = timer::timer_get_usec();
    while (curr - start < timeout) {
        unsafe {
            if uart::rx_has_byte() {
                return Ok(());
            }
            curr = timer::timer_get_usec();
        }
    }

    Err(BOOTERROR::TIMEOUT)
}

#[no_mangle]
// waits for data for timeout usecs
pub fn wait_for_data(timeout: u32) {
    boot_put32(GET_PROG_INFO);
    while let Err(data) = has_data_timeout(timeout) {
        boot_put32(GET_PROG_INFO);
    }
}

#[no_mangle]
pub fn delay(ncycles: u32) {
    let mut cnt = ncycles;
    while cnt > 0 {
        rpi::nop();
        cnt -= 1;
    }
}

#[no_mangle]
pub fn get_code() -> Result<u32, BOOTERROR> {
    wait_for_data(500 * 1000);

    // expect [PUT_PROG_INFO, addr, nbytes, cksum]
    let pi_code: u32 = boot_get32();
    let pi_addr: u32 = boot_get32();
    let pi_nbytes: u32 = boot_get32();
    let pi_cksum: u32 = boot_get32();

    // check that  we got PUT_PROG_INFO and that start  addr is right
    if pi_code != PUT_PROG_INFO {
        boot_putk("pi_code is not PUT_PROG_INFO.");
        return Err(BOOTERROR::OTHER);
    }

    if pi_addr != ARMBASE {
        boot_putk("pi_addr is not  ARMBASE");
        return Err(BOOTERROR::BAD_ADDR);
    }

    //check  if binary will collide with bootloader  code, if it will return proper err
    let pi_end_addr = pi_addr + pi_nbytes;
    if pi_end_addr >= BOOTLOADER_START {
        return Err(BOOTERROR::BAD_ADDR);
    }

    // send GET_CODE and cksum back
    boot_put32(GET_CODE);
    boot_put32(pi_cksum);

    // expect [PUT_CODE, <code>]
    // read each  byte and write it startin at addr
    // using PUT8
    let pi_put = boot_get32();
    for i in 0..pi_nbytes {
        unsafe {
            rpi::PUT8((pi_addr + i) as usize, uart::read_byte() as u8);
        }
    }
    if pi_put != PUT_CODE {
        boot_putk("PUT_CODE does not match value of pi_code.");
        return Err(BOOTERROR::OTHER);
    }

    //verify checksum
    let mut consumeable_addr = pi_addr.clone();
    let new_cksum = crc32::crc32(ptr::addr_of_mut!(consumeable_addr) as *mut u8, pi_nbytes);
    if pi_cksum != new_cksum {
        boot_putk("Checksums do not match.");
        return Err(BOOTERROR::BAD_CKSUM);
    }

    //send back boot success
    boot_put32(BOOT_SUCCESS);
    Ok(pi_addr)
}