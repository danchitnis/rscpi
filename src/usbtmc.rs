/* https://github.com/rogerioadris/rust-usbtmc/blob/main/src/instrument.rs
*
*/

use crate::Usbtmc;
use byteorder::{ByteOrder, LittleEndian};
use futures_lite::future::block_on;
use nusb::transfer::RequestBuffer;
use nusb::transfer::TransferError;

const USBTMC_MSGID_DEV_DEP_MSG_OUT: u8 = 1;
const USBTMC_MSGID_DEV_DEP_MSG_IN: u8 = 2;

#[derive(Debug)]
pub enum UsbtmcErrors {
    BulkOutTransferError,
    BulkInTransferError,
    InvalidData,
}

macro_rules! log {
    // The `$(...)*` syntax is used to match against any number of arguments of any type
    ($($arg:tt)*) => {
        // Check if in debug mode and call `print!` if true
        if cfg!(debug_assertions) {
            print!($($arg)*);
        }
    };
}

#[cfg(debug_assertions)]
fn print_array_partial(bytes: Vec<u8>) {
    let len = bytes.len();
    if len <= 25 {
        for byte in &bytes {
            print!("{} ", byte);
        }
        println!(); // Print a newline for neatness
    } else {
        print!("[");
        for byte in bytes.iter().take(20) {
            print!("{} ", byte);
        }
        print!(" ... ");
        for byte in bytes.iter().skip(len - 5) {
            print!("{} ", byte);
        }
        println!("]");
    }
}

fn little_write_u32(size: u32, len: u8) -> Vec<u8> {
    let mut buf = vec![0; len as usize];
    LittleEndian::write_u32(&mut buf, size);

    buf
}

/*
* USBTMC document Table 1
*/
fn pack_bulk_out_header(msgid: u8, btag: u8) -> Vec<u8> {
    //let last_btag: u8 = 0x00;
    let btag: u8 = (btag % 255) + 1;
    //last_btag = btag;

    // BBBx
    vec![msgid, btag, !btag, 0x00]
}

/*
* USBTMC document Table 3 and USB488 document Table 3
*/
fn pack_dev_dep_msg_out_header(transfer_size: usize, eom: bool, btag: u8) -> Vec<u8> {
    let mut header = pack_bulk_out_header(USBTMC_MSGID_DEV_DEP_MSG_OUT, btag);

    let mut total_transfer_size: Vec<u8> = little_write_u32(transfer_size as u32, 4);
    let bm_transfer_attributes: u8 = if eom { 0x01 } else { 0x00 };

    header.append(&mut total_transfer_size);
    header.push(bm_transfer_attributes);
    header.append(&mut vec![0x00; 3]);

    // check if transfer size is not 0

    header
}

/*
* USBTMC document Table 4
*/
fn pack_dev_dep_msg_in_header(transfer_size: usize, term_char: u8) -> Vec<u8> {
    let mut header = pack_bulk_out_header(USBTMC_MSGID_DEV_DEP_MSG_IN, 0x00);

    let mut max_transfer_size: Vec<u8> = little_write_u32(transfer_size as u32, 4);
    let bm_transfer_attributes: u8 = if term_char == 0 { 0x00 } else { 0x02 };

    header.append(&mut max_transfer_size);
    header.push(bm_transfer_attributes);
    header.push(term_char);
    header.append(&mut vec![0x00; 2]);

    header
}

fn is_query(data: &[u8]) -> bool {
    // Define the byte you are looking for
    let question_mark = b'?';

    // Use the contains method to check if the data contains the byte
    data.contains(&question_mark)
}

fn read_data_transfer(
    usbtmc: &mut Usbtmc,
    big_buffer: &mut Vec<u8>,
) -> Result<usize, UsbtmcErrors> {
    let recv_buffer_size = usbtmc.endpoint_in_max_packet_size * 1024;
    let request_buffer = RequestBuffer::new(recv_buffer_size);
    let okr_result = block_on(
        usbtmc
            .interface
            .bulk_in(usbtmc.endpoint_in_addr, request_buffer),
    )
    .into_result();

    let okr = okr_result.map_err(|_| UsbtmcErrors::BulkInTransferError)?;

    log!("okr->: ");

    #[cfg(debug_assertions)]
    print_array_partial(okr.to_vec());

    big_buffer.extend_from_slice(&okr);

    let usb_packet_size = okr.len();
    log!("usb packet size: {}\n", usb_packet_size);

    Ok(usb_packet_size)
}

fn read_data(usbtmc: &mut Usbtmc, big_big_buffer: &mut Vec<u8>) -> Result<bool, UsbtmcErrors> {
    let max_transfer_size: usize = 1024 * usbtmc.endpoint_in_max_packet_size;
    let mut big_buffer: Vec<u8> = Vec::new();

    let buffer_size = max_transfer_size;

    let send = pack_dev_dep_msg_in_header(max_transfer_size, 0);
    let ok2_results =
        block_on(usbtmc.interface.bulk_out(usbtmc.endpoint_out_addr, send)).into_result();

    let ok2 = ok2_results.map_err(|_| UsbtmcErrors::BulkOutTransferError)?;

    log!("ok2->: {:?}\n", ok2);

    let mut usb_packet_recv_size = read_data_transfer(usbtmc, &mut big_buffer)?;

    // Separate the bytes
    let (header, _): (&[u8], &[u8]) = big_buffer.split_at(12);

    // Convert the first part to hexadecimal values and print
    log!("Header Hex values: ");
    #[cfg(debug_assertions)]
    for byte in header {
        log!("{:02x} ", byte);
    }
    log!("\n");

    let payload_size = LittleEndian::read_u32(&header[4..8]) as usize;
    log!("Payload size in header: {}\n", payload_size);

    let eom: bool = header[8] == 0x01;
    log!("EOM: {}\n", eom);

    while usb_packet_recv_size == buffer_size {
        log!("Packet size equals buffer size. Reading more data.\n");
        usb_packet_recv_size = read_data_transfer(usbtmc, &mut big_buffer)?;
    }

    big_big_buffer.extend_from_slice(&big_buffer[12..]);

    Ok(eom)
}

pub fn write_str(usbtmc: &mut Usbtmc, command_data: &str) -> Result<(), UsbtmcErrors> {
    let offset: usize = 0;
    let num: usize = command_data.len();

    let data = &command_data[offset..(num - offset)];
    write_binary(usbtmc, data.as_bytes())
}

pub fn write_binary(usbtmc: &mut Usbtmc, in_data: &[u8]) -> Result<(), UsbtmcErrors> {
    let interface = &mut usbtmc.interface;

    let mut size: usize = in_data.len();
    let max_data_size: usize = 1024 * usbtmc.endpoint_out_max_packet_size;

    let mut data = in_data;

    let mut btag: u8 = 0x00;

    while size > max_data_size {
        let send = pack_dev_dep_msg_out_header(max_data_size, false, btag);
        let mut b: Vec<u8> = data[0..max_data_size].to_vec();
        let mut req = send;
        req.append(&mut b);
        req.append(&mut vec![0x00; (4 - (max_data_size % 4)) % 4]);

        let ok_results: Result<nusb::transfer::ResponseBuffer, TransferError> =
            block_on(interface.bulk_out(usbtmc.endpoint_out_addr, req)).into_result();

        let ok = ok_results.map_err(|_| UsbtmcErrors::BulkOutTransferError)?;

        log!("ok->: {:?}\n", ok);

        size -= max_data_size;

        data = &data[max_data_size..];

        btag = (btag % 255) + 1;
    }

    let mut req = pack_dev_dep_msg_out_header(size, true, btag);
    let mut b: Vec<u8> = data.to_vec();
    req.append(&mut b);
    req.append(&mut vec![0x00; (4 - (size % 4)) % 4]);

    let ok_results: Result<nusb::transfer::ResponseBuffer, TransferError> =
        block_on(interface.bulk_out(usbtmc.endpoint_out_addr, req)).into_result();

    let ok = ok_results.map_err(|_| UsbtmcErrors::BulkOutTransferError)?;

    log!("ok->: {:?}\n", ok);

    Ok(())
}

pub fn send_command_raw_binary(
    usbtmc: &mut Usbtmc,
    data: &[u8],
    query: bool,
) -> Result<Vec<u8>, UsbtmcErrors> {
    write_binary(usbtmc, data)?;

    let mut big_big_buffer: Vec<u8> = Vec::new();

    if query {
        log!("query detected\n");
        let mut eom: bool = read_data(usbtmc, &mut big_big_buffer)?;

        while !eom {
            log!("eom is false. Reading more data.\n");
            eom = read_data(usbtmc, &mut big_big_buffer)?;
        }
        log!(
            "transfer complete. total payload size: {}\n",
            big_big_buffer.len()
        );
        let term_recv: bool = big_big_buffer[big_big_buffer.len() - 1] == 0x0A; //=10
        log!("term_recv: {}\n", term_recv);

        if !term_recv {
            log!("term_recv is false. Reading more data.\n");
            return Err(UsbtmcErrors::InvalidData);
        }

        #[cfg(debug_assertions)]
        if big_big_buffer.len() > 100 {
            log!("The first 10 bytes of the payload:\n");
            let ten_bytes = &big_big_buffer[0..10];
            let ascii_string: String = ten_bytes.iter().map(|&b| b as char).collect();
            log!("{}\n", ascii_string);
        }

        big_big_buffer.truncate(big_big_buffer.len() - 1);
    } else {
        log!("No command detected. exiting.\n");
    }
    log!("\n");

    Ok(big_big_buffer)
}

pub(crate) fn send_command_raw(
    usbtmc: &mut Usbtmc,
    command: &str,
) -> Result<Vec<u8>, UsbtmcErrors> {
    let command_with_newline = command.to_owned() + "\n";
    let command_data = command_with_newline.as_bytes();

    log!("Sending command: {:?}\n", command);
    let query = is_query(command_data);

    send_command_raw_binary(usbtmc, command_data, query)
}

pub(crate) fn send_command(usbtmc: &mut Usbtmc, command: &str) -> Result<String, UsbtmcErrors> {
    let data: Vec<u8> = send_command_raw(usbtmc, command)?;

    let ascii_string: String = data.iter().map(|&b| b as char).collect();

    Ok(ascii_string)
}
