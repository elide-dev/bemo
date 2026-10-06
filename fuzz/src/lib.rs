use bemo::abi::*;
use bemo::buffer::{Budget, Buffer};
use bemo::http::{HttpConnection, Outcome};
use std::collections::VecDeque;

#[derive(Debug, PartialEq, Eq)]
enum Record {
  Head(Vec<u8>, u8, bool),
  Data(Vec<u8>),
  End,
  Error(u16),
}

fn parse(data: &[u8], chunk: usize) -> Vec<Record> {
  let budget = Budget::new(2 * 1024 * 1024);
  let mut connection = HttpConnection::new(budget.clone());
  let mut records = Vec::new();
  let mut outcomes = VecDeque::new();
  for bytes in data.chunks(chunk.max(1)) {
    let mut buffer = Buffer::new(bytes.len(), budget.clone()).unwrap();
    buffer.write(0, bytes).unwrap();
    connection.ingest(buffer, &mut outcomes);
    while let Some(outcome) = outcomes.pop_front() {
      match outcome {
        Outcome::Request(head) => {
          // Access every foreign-visible span while its receive allocation is retained.
          assert!(!head.method_bytes().is_empty());
          let _ = head.path_bytes();
          for index in 0..head.headers.len() {
            assert!(head.header_name(index).is_some());
            assert!(head.header_value(index).is_some());
          }
          records.push(Record::Head(head.head.as_ref().to_vec(), head.version, head.keep_alive));
        }
        Outcome::Segment(bytes) => {
          if let Some(Record::Data(previous)) = records.last_mut() {
            previous.extend_from_slice(bytes.as_ref());
          } else {
            records.push(Record::Data(bytes.as_ref().to_vec()));
          }
        }
        Outcome::BodyEnd => records.push(Record::End),
        Outcome::Error(error) => records.push(Record::Error(error.status())),
      }
    }
  }
  drop(connection);
  assert_eq!(budget.used(), 0);
  records
}

/// Fragmentation must preserve parsed heads, body bytes, framing, and errors.
pub fn http(data: &[u8]) {
  if data.is_empty() || data.len() > 32 * 1024 {
    return;
  }
  let wire = &data[1..];
  assert_eq!(parse(wire, wire.len()), parse(wire, usize::from(data[0]) + 1));
}

/// Retained and split views preserve bytes and release their capacity exactly once.
pub fn buffers(data: &[u8]) {
  if data.is_empty() || data.len() > 32 * 1024 {
    return;
  }
  let budget = Budget::new(data.len());
  let mut buffer = Buffer::new(data.len(), budget.clone()).unwrap();
  buffer.write(0, data).unwrap();
  let at = usize::from(data[0]) % (data.len() + 1);
  let frozen = buffer.freeze();
  let retained = frozen.clone();
  let tail = frozen.slice(at..data.len()).unwrap();
  assert!(frozen.clone().try_into_mut().is_err());
  drop(frozen);
  assert_eq!(tail.as_ref(), &data[at..]);
  assert_eq!(budget.used(), data.len());
  drop(tail);
  let mut buffer = retained.try_into_mut().unwrap();
  buffer.write(0, &[data[0].wrapping_add(1)]).unwrap();
  assert_eq!(buffer.freeze().as_ref()[0], data[0].wrapping_add(1));
  assert_eq!(budget.used(), 0);
}

/// Generate valid handle operations plus stale-handle and admission failures.
pub fn abi(data: &[u8]) {
  let owner = elide_transport_owner_new(4096);
  assert_ne!(owner, 0);
  let mut handles = Vec::new();
  let mut retired = Vec::new();
  for instruction in data.as_chunks::<3>().0.iter().take(256) {
    match instruction[0] % 8 {
      0 if handles.len() < 32 => {
        let handle = elide_transport_buffer_new(owner, u64::from(instruction[1]) + 1);
        if handle != 0 {
          // SAFETY: Buffer allocation initializes capacity and there are no concurrent writers.
          assert_eq!(
            unsafe { elide_transport_buffer_freeze(handle, u64::from(instruction[1]) + 1) },
            0
          );
          handles.push(handle);
        }
      }
      1 if !handles.is_empty() && handles.len() < 32 => {
        let handle = handles[usize::from(instruction[1]) % handles.len()];
        let slice = elide_transport_buffer_slice(handle, u64::from(instruction[1]), u64::from(instruction[2]));
        if slice != 0 {
          handles.push(slice);
        }
      }
      2 if !handles.is_empty() => {
        let handle = handles.swap_remove(usize::from(instruction[1]) % handles.len());
        assert_eq!(elide_transport_buffer_release(handle), 0);
        retired.push(handle);
      }
      3 if !retired.is_empty() => {
        assert_eq!(
          elide_transport_buffer_release(retired[usize::from(instruction[1]) % retired.len()]),
          INVALID
        );
      }
      4 => assert!(elide_transport_owner_used(owner) <= 4096),
      5 => {
        assert_eq!(
          // SAFETY: The instruction is live initialized storage; invalid driver/workload reject admission.
          unsafe { elide_transport_socket_send_inline(0, 0, 0, instruction.as_ptr(), instruction.len() as u64) },
          -1
        );
      }
      6 => {
        let handle = handles
          .get(usize::from(instruction[1]) % handles.len().max(1))
          .copied()
          .unwrap_or(0);
        let regions = [handle, u64::from(instruction[1]), u64::from(instruction[2])];
        let count = [0, 1, 65][usize::from(instruction[2]) % 3];
        assert_eq!(
          // SAFETY: Count 1 reads exactly the live triple; invalid counts reject before access.
          unsafe { elide_transport_socket_send_gathered(0, 0, 0, regions.as_ptr(), count) },
          0
        );
      }
      7 => {
        let mut output = ReceiveResult::default();
        assert_eq!(
          // SAFETY: The descriptor is aligned and writable; invalid handles reject admission.
          unsafe { elide_transport_socket_receive_new_result(0, 0, 0, owner, u64::from(instruction[1]), &mut output) },
          INVALID
        );
        assert_eq!(output.buffer, 0);
        assert!(output.address.is_null());
      }
      _ => {}
    }
  }
  for handle in handles {
    assert_eq!(elide_transport_buffer_release(handle), 0);
  }
  assert_eq!(elide_transport_owner_used(owner), 0);
  assert_eq!(elide_transport_owner_release(owner), 0);
  assert_eq!(elide_transport_buffer_new(owner, 1), 0);
}
