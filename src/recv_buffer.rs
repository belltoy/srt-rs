use bytes::{Bytes, BytesMut};
use std::{collections::VecDeque, fmt};

use crate::{packet::PacketLocation, DataPacket, SeqNumber};

pub struct RecvBuffer {
    // stores the incoming packets as they arrive
    // `buffer[0]` will hold sequence number `head`
    buffer: VecDeque<Option<DataPacket>>,

    // The next to be released sequence number
    head: SeqNumber,
}

impl RecvBuffer {
    /// Creates a `RecvBuffer`
    ///
    /// * `head` - The sequence number of the next packet
    pub fn new(head: SeqNumber) -> RecvBuffer {
        RecvBuffer {
            buffer: VecDeque::new(),
            head,
        }
    }

    /// The next to be released sequence number
    pub fn next_release(&self) -> SeqNumber {
        self.head
    }

    /// Adds a packet to the buffer
    /// If `pack.seq_number < self.head`, this is nop (ie it appears before an already released packet)
    pub fn add(&mut self, pack: DataPacket) {
        if pack.seq_number < self.head {
            return;
        }

        // resize `buffer` if necessary
        let idx = (pack.seq_number - self.head) as usize;
        if idx >= self.buffer.len() {
            self.buffer.resize(idx + 1, None);
        }

        // add the new element
        self.buffer[idx] = Some(pack)
    }

    /// Check if the next message is available. Returns `None` if there is no message,
    /// and `Some(i)` if there is a message available, where `i` is the number of packets this message spans
    pub fn next_msg_ready(&self) -> Option<usize> {
        let first = self.buffer.front();
        if let Some(Some(first)) = first {
            // we have a first packet, make sure it has the start flag set
            assert!(first.message_loc.contains(PacketLocation::FIRST));

            let mut count = 1;

            for i in &self.buffer {
                match i {
                    Some(ref pack) if pack.message_loc.contains(PacketLocation::LAST) => {
                        return Some(count)
                    }
                    None => return None,
                    _ => count += 1,
                }
            }
        }

        None
    }

    /// Check if there is an available message, returning it if found
    pub fn next_msg(&mut self) -> Option<Bytes> {
        let count = self.next_msg_ready()?;

        self.head += count as u32;

        // optimize for single packet messages
        if count == 1 {
            return Some(self.buffer.pop_front().unwrap().unwrap().payload.clone());
        }

        // accumulate the rest
        Some(
            self.buffer
                .drain(0..count)
                .fold(BytesMut::new(), |mut bytes, pack| {
                    bytes.extend(pack.unwrap().payload);
                    bytes
                })
                .freeze(),
        )
    }
}

impl fmt::Debug for RecvBuffer {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(
            f,
            "{:?}",
            self.buffer
                .iter()
                .map(|o| o
                    .as_ref()
                    .map(|pack| (pack.seq_number.as_raw(), pack.message_loc)))
                .collect::<Vec<_>>()
        )
    }
}

#[cfg(test)]
mod test {

    use super::RecvBuffer;
    use bytes::Bytes;
    use {packet::PacketLocation, DataPacket, MsgNumber, SeqNumber, SocketID};

    fn basic_pack() -> DataPacket {
        DataPacket {
            seq_number: SeqNumber::new(5),
            message_loc: PacketLocation::FIRST,
            in_order_delivery: false,
            message_number: MsgNumber(0),
            timestamp: 0,
            dest_sockid: SocketID(4),
            payload: Bytes::new(),
        }
    }

    #[test]
    fn not_ready_empty() {
        let mut buf = RecvBuffer::new(SeqNumber::new(3));

        assert_eq!(buf.next_msg_ready(), None);
        assert_eq!(buf.next_msg(), None);
        assert_eq!(buf.next_release(), SeqNumber(3));
    }

    #[test]
    fn not_ready_no_more() {
        let mut buf = RecvBuffer::new(SeqNumber::new(5));
        buf.add(DataPacket {
            seq_number: SeqNumber(5),
            message_loc: PacketLocation::FIRST,
            ..basic_pack()
        });

        assert_eq!(buf.next_msg_ready(), None);
        assert_eq!(buf.next_msg(), None);
        assert_eq!(buf.next_release(), SeqNumber(5));
    }

    #[test]
    fn not_ready_none() {
        let mut buf = RecvBuffer::new(SeqNumber::new(5));
        buf.add(DataPacket {
            seq_number: SeqNumber(5),
            message_loc: PacketLocation::FIRST,
            ..basic_pack()
        });
        buf.add(DataPacket {
            seq_number: SeqNumber(7),
            message_loc: PacketLocation::FIRST,
            ..basic_pack()
        });

        assert_eq!(buf.next_msg_ready(), None);
        assert_eq!(buf.next_msg(), None);
        assert_eq!(buf.next_release(), SeqNumber(5));
    }

    #[test]
    fn not_ready_middle() {
        let mut buf = RecvBuffer::new(SeqNumber::new(5));
        buf.add(DataPacket {
            seq_number: SeqNumber(5),
            message_loc: PacketLocation::FIRST,
            ..basic_pack()
        });
        buf.add(DataPacket {
            seq_number: SeqNumber(6),
            message_loc: PacketLocation::empty(),
            ..basic_pack()
        });

        assert_eq!(buf.next_msg_ready(), None);
        assert_eq!(buf.next_msg(), None);
        assert_eq!(buf.next_release(), SeqNumber(5));
    }

    #[test]
    fn ready_single() {
        let mut buf = RecvBuffer::new(SeqNumber::new(5));
        buf.add(DataPacket {
            seq_number: SeqNumber(5),
            message_loc: PacketLocation::FIRST | PacketLocation::LAST,
            payload: From::from(&b"hello"[..]),
            ..basic_pack()
        });
        buf.add(DataPacket {
            seq_number: SeqNumber(6),
            message_loc: PacketLocation::empty(),
            payload: From::from(&b"no"[..]),
            ..basic_pack()
        });

        assert_eq!(buf.next_msg_ready(), Some(1));
        assert_eq!(buf.next_msg(), Some(From::from(&b"hello"[..])));
        assert_eq!(buf.next_release(), SeqNumber(6));
        assert_eq!(buf.buffer.len(), 1);
    }

    #[test]
    fn ready_multi() {
        let mut buf = RecvBuffer::new(SeqNumber::new(5));
        buf.add(DataPacket {
            seq_number: SeqNumber(5),
            message_loc: PacketLocation::FIRST,
            payload: From::from(&b"hello"[..]),
            ..basic_pack()
        });
        buf.add(DataPacket {
            seq_number: SeqNumber(6),
            message_loc: PacketLocation::empty(),
            payload: From::from(&b"yas"[..]),
            ..basic_pack()
        });
        buf.add(DataPacket {
            seq_number: SeqNumber(7),
            message_loc: PacketLocation::LAST,
            payload: From::from(&b"nas"[..]),
            ..basic_pack()
        });

        assert_eq!(buf.next_msg_ready(), Some(3));
        assert_eq!(buf.next_msg(), Some(From::from(&b"helloyasnas"[..])));
        assert_eq!(buf.next_release(), SeqNumber(8));
        assert_eq!(buf.buffer.len(), 0);
    }

}
