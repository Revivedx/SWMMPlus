// A local header whose Zip64 field holds only the header offset, as the zip crate (2.x) writes it for entries that
// start past 4 GiB: piz 0.5.1 took the 8 bytes for a disk number and refused the whole archive.

fn archive() -> Vec<u8> {
    let name = b"a.txt";
    let data = b"hello";
    let crc = crc32fast::hash(data);
    let mut z = Vec::new();
    // local header, with a Zip64 field of 8 bytes (the header offset) though no size needs it
    z.extend(0x04034b50u32.to_le_bytes());
    z.extend(45u16.to_le_bytes()); // version needed
    z.extend(0u16.to_le_bytes()); // flags
    z.extend(0u16.to_le_bytes()); // stored
    z.extend(0u16.to_le_bytes()); // time
    z.extend(0x21u16.to_le_bytes()); // date: 1980-01-01
    z.extend(crc.to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((name.len() as u16).to_le_bytes());
    z.extend(12u16.to_le_bytes()); // extra field length
    z.extend(name);
    z.extend(1u16.to_le_bytes()); // Zip64 extended information
    z.extend(8u16.to_le_bytes());
    z.extend(0u64.to_le_bytes()); // the header offset
    z.extend(data);
    // central directory entry, without a Zip64 field
    let cd_start = z.len();
    z.extend(0x02014b50u32.to_le_bytes());
    z.extend(45u16.to_le_bytes()); // version made by
    z.extend(45u16.to_le_bytes()); // version needed
    z.extend(0u16.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    z.extend(0x21u16.to_le_bytes());
    z.extend(crc.to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((data.len() as u32).to_le_bytes());
    z.extend((name.len() as u16).to_le_bytes());
    z.extend(0u16.to_le_bytes()); // extra
    z.extend(0u16.to_le_bytes()); // comment
    z.extend(0u16.to_le_bytes()); // disk
    z.extend(0u16.to_le_bytes()); // internal attributes
    z.extend(0u32.to_le_bytes()); // external attributes
    z.extend(0u32.to_le_bytes()); // header offset
    z.extend(name);
    let cd_size = z.len() - cd_start;
    // end of central directory
    z.extend(0x06054b50u32.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    z.extend(0u16.to_le_bytes());
    z.extend(1u16.to_le_bytes());
    z.extend(1u16.to_le_bytes());
    z.extend((cd_size as u32).to_le_bytes());
    z.extend((cd_start as u32).to_le_bytes());
    z.extend(0u16.to_le_bytes());
    z
}

#[test]
fn reads_an_entry_whose_local_header_repeats_its_offset() {
    let bytes = archive();
    let zip = piz::ZipArchive::new(&bytes).expect("the archive opens");
    let entry = zip.entries().iter().find(|e| e.path.as_str() == "a.txt").expect("the entry is listed");
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut zip.read(entry).expect("the entry reads"), &mut out).unwrap();
    assert_eq!(out, b"hello");
}
