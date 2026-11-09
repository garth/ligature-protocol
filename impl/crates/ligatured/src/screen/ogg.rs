//! Ogg Opus, read and written by hand (milestone 9, LP-200). A share's sound
//! travels as bare Opus packets; ffmpeg, which encodes and decodes them on
//! both desktops, speaks Ogg, so the daemon takes the packets out of the Ogg
//! pages ffmpeg writes and `lpview` puts them back into pages for the ffmpeg
//! that plays them. The two header packets (`OpusHead`, `OpusTags`) are
//! written here and skipped there. RFC 3533 (Ogg) and RFC 7845 (Ogg Opus).

/// Samples per 20 ms packet at 48 kHz.
pub const SAMPLES_PER_PACKET: u64 = 960;

/// The Ogg page CRC: polynomial 0x04c11db7, no reflection, no final XOR.
#[must_use]
pub fn crc(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for &b in bytes {
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Writes Ogg Opus pages, one packet a page, for a player that reads Ogg.
#[derive(Debug)]
pub struct Writer {
    serial: u32,
    sequence: u32,
    granule: u64,
}

impl Writer {
    /// A writer whose first pages carry the Opus headers for `channels`.
    #[must_use]
    pub fn new(serial: u32) -> Self {
        Self {
            serial,
            sequence: 0,
            granule: 0,
        }
    }

    /// The two header pages: `OpusHead` (beginning of stream) and
    /// `OpusTags`.
    #[must_use]
    pub fn headers(&mut self, channels: u8) -> Vec<u8> {
        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead");
        head.push(1); // version
        head.push(channels);
        head.extend_from_slice(&0u16.to_le_bytes()); // pre-skip
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes()); // output gain
        head.push(0); // channel mapping family: mono or stereo
        let mut tags = Vec::new();
        tags.extend_from_slice(b"OpusTags");
        let vendor = b"ligature";
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes());
        let mut out = self.page(&head, 0x02, 0);
        out.extend(self.page(&tags, 0, 0));
        out
    }

    /// One page carrying one 20 ms packet.
    #[must_use]
    pub fn packet(&mut self, packet: &[u8]) -> Vec<u8> {
        self.granule += SAMPLES_PER_PACKET;
        self.page(packet, 0, self.granule)
    }

    fn page(&mut self, packet: &[u8], flags: u8, granule: u64) -> Vec<u8> {
        let mut lacing = Vec::new();
        let mut left = packet.len();
        loop {
            if left >= 255 {
                lacing.push(255u8);
                left -= 255;
            } else {
                lacing.push(left as u8);
                break;
            }
        }
        let mut page = Vec::with_capacity(27 + lacing.len() + packet.len());
        page.extend_from_slice(b"OggS");
        page.push(0);
        page.push(flags);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&self.serial.to_le_bytes());
        page.extend_from_slice(&self.sequence.to_le_bytes());
        page.extend_from_slice(&[0, 0, 0, 0]);
        page.push(lacing.len() as u8);
        page.extend_from_slice(&lacing);
        page.extend_from_slice(packet);
        let sum = crc(&page).to_le_bytes();
        page[22..26].copy_from_slice(&sum);
        self.sequence = self.sequence.wrapping_add(1);
        page
    }
}

/// Reads the Opus packets out of a stream of Ogg pages, as ffmpeg writes
/// them, the two header packets left out and a packet continued across pages
/// joined.
#[derive(Debug, Default)]
pub struct Reader {
    buffer: Vec<u8>,
    /// A packet whose last lacing value was 255: continued on the next page.
    partial: Vec<u8>,
    headers_seen: u8,
}

impl Reader {
    /// Takes more bytes; returns the whole packets they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(bytes);
        let mut packets = Vec::new();
        loop {
            let Some(start) = self.buffer.windows(4).position(|w| w == b"OggS") else {
                // Keep the tail that might begin a capture pattern.
                let keep = self.buffer.len().min(3);
                let from = self.buffer.len() - keep;
                self.buffer.drain(..from);
                break;
            };
            if start > 0 {
                self.buffer.drain(..start);
            }
            if self.buffer.len() < 27 {
                break;
            }
            let segments = usize::from(self.buffer[26]);
            let header_len = 27 + segments;
            if self.buffer.len() < header_len {
                break;
            }
            let lacing: Vec<u8> = self.buffer[27..header_len].to_vec();
            let body_len: usize = lacing.iter().map(|&l| usize::from(l)).sum();
            if self.buffer.len() < header_len + body_len {
                break;
            }
            let body: Vec<u8> = self.buffer[header_len..header_len + body_len].to_vec();
            self.buffer.drain(..header_len + body_len);
            let mut at = 0usize;
            for &l in &lacing {
                let l = usize::from(l);
                self.partial.extend_from_slice(&body[at..at + l]);
                at += l;
                if l < 255 {
                    let packet = std::mem::take(&mut self.partial);
                    if self.headers_seen < 2
                        && (packet.starts_with(b"OpusHead") || packet.starts_with(b"OpusTags"))
                    {
                        self.headers_seen += 1;
                        continue;
                    }
                    if !packet.is_empty() {
                        packets.push(packet);
                    }
                }
            }
        }
        packets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_matches_the_ogg_reference() {
        // The CRC of the empty string is zero and of a known page header is
        // whatever the reference gives; what matters here is that a page we
        // write passes a reader that checks it, which ffmpeg does, and that a
        // changed byte changes the sum.
        assert_eq!(crc(b""), 0);
        assert_ne!(crc(b"OggS"), crc(b"OggT"));
        // RFC 3533's polynomial on one byte: 0x01 shifts to the polynomial.
        assert_eq!(crc(&[0x01]), 0x04c1_1db7);
    }

    #[test]
    fn pages_written_are_read_back_without_the_headers() {
        let mut w = Writer::new(7);
        let mut bytes = w.headers(2);
        let packets: Vec<Vec<u8>> = vec![vec![0xfc; 3], vec![0xfd; 300], vec![1]];
        for p in &packets {
            bytes.extend(w.packet(p));
        }
        // Fed in uneven pieces.
        let mut r = Reader::default();
        let mut got = Vec::new();
        for piece in bytes.chunks(37) {
            got.extend(r.push(piece));
        }
        assert_eq!(got, packets);
        // The pages themselves are well formed: the capture pattern, version
        // 0, the headers flagged as the stream's beginning, rising sequence
        // numbers and granules of 960 a packet, and a CRC that holds.
        assert_eq!(&bytes[0..4], b"OggS");
        assert_eq!(bytes[4], 0);
        assert_eq!(bytes[5], 0x02);
        let mut pos = 0;
        let mut seq = Vec::new();
        let mut granules = Vec::new();
        while pos < bytes.len() {
            let segs = usize::from(bytes[pos + 26]);
            let body: usize = bytes[pos + 27..pos + 27 + segs]
                .iter()
                .map(|&l| usize::from(l))
                .sum();
            let end = pos + 27 + segs + body;
            let mut page = bytes[pos..end].to_vec();
            let stated = u32::from_le_bytes(page[22..26].try_into().unwrap());
            page[22..26].copy_from_slice(&[0; 4]);
            assert_eq!(crc(&page), stated);
            seq.push(u32::from_le_bytes(page[18..22].try_into().unwrap()));
            granules.push(u64::from_le_bytes(page[6..14].try_into().unwrap()));
            pos = end;
        }
        assert_eq!(seq, [0, 1, 2, 3, 4]);
        assert_eq!(granules, [0, 0, 960, 1920, 2880]);
    }

    /// A page with the given lacing values and body, as another writer might
    /// make it.
    fn raw_page(lacing: &[u8], body: &[u8], sequence: u32) -> Vec<u8> {
        let mut page = Vec::new();
        page.extend_from_slice(b"OggS");
        page.extend_from_slice(&[0, 0]);
        page.extend_from_slice(&0u64.to_le_bytes());
        page.extend_from_slice(&1u32.to_le_bytes());
        page.extend_from_slice(&sequence.to_le_bytes());
        page.extend_from_slice(&[0; 4]);
        page.push(lacing.len() as u8);
        page.extend_from_slice(lacing);
        page.extend_from_slice(body);
        let sum = crc(&page).to_le_bytes();
        page[22..26].copy_from_slice(&sum);
        page
    }

    #[test]
    fn a_packet_continued_across_pages_is_joined_and_junk_is_skipped() {
        // A page ending in a 255-byte lacing value says the packet goes on in
        // the next page, whose first lacing value ends it.
        let first = raw_page(&[255], &[9u8; 255], 0);
        let second = raw_page(&[10, 2], &[[8u8; 10].as_slice(), &[7, 7]].concat(), 1);
        let mut r = Reader::default();
        let mut bytes = b"junk".to_vec();
        bytes.extend(first);
        bytes.extend(second);
        let got = r.push(&bytes);
        let mut joined = vec![9u8; 255];
        joined.extend([8u8; 10]);
        assert_eq!(got, vec![joined, vec![7, 7]]);
        // The writer itself ends a 255-byte packet within its page.
        let mut w = Writer::new(1);
        let page = w.packet(&[9u8; 255]);
        assert_eq!(page[26], 2, "two lacing values");
        assert_eq!(&page[27..29], &[255, 0]);
    }
}
