use std::collections::VecDeque;
use std::hint::black_box;

use bemo::buffer::{Budget, Buffer};
use bemo::http::{DateCache, HttpConnection, Outcome, ResponseHeader, encode_response};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

fn http(c: &mut Criterion) {
  let mut group = c.benchmark_group("http1/parse");
  for (name, request) in [
    (
      "get",
      b"GET /resource?q=1 HTTP/1.1\r\nHost: localhost\r\nAccept: */*\r\n\r\n".as_slice(),
    ),
    (
      "post",
      b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\r\npayload!".as_slice(),
    ),
    (
      "chunked",
      b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n8\r\npayload!\r\n0\r\n\r\n".as_slice(),
    ),
  ] {
    for fragment in [request.len(), 7] {
      group.throughput(Throughput::Elements(1));
      group.bench_function(BenchmarkId::new(name, fragment), |b| {
        let budget = Budget::new(1024 * 1024);
        b.iter(|| {
          let mut parser = HttpConnection::new(budget.clone());
          let mut events = VecDeque::new();
          for bytes in black_box(request).chunks(fragment) {
            let mut buffer = Buffer::new(bytes.len(), budget.clone()).unwrap();
            buffer.write(0, bytes).unwrap();
            parser.ingest(buffer, &mut events);
          }
          assert_eq!(events.iter().filter(|e| matches!(e, Outcome::Request(_))).count(), 1);
          assert!(!events.iter().any(|e| matches!(e, Outcome::Error(_))));
          black_box(events);
        });
        assert_eq!(budget.used(), 0);
      });
    }
  }
  group.finish();
  let mut group = c.benchmark_group("http1/encode");
  for size in [0, 1024, 65536] {
    let body = vec![b'x'; size];
    let budget = Budget::new(1024 * 1024);
    let mut date = DateCache::default();
    black_box(date.line());
    let headers = [ResponseHeader {
      name: b"content-type",
      value: b"application/octet-stream",
    }];
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::new("response", size), |b| {
      b.iter(|| {
        let response = encode_response(&budget, &mut date, 1, 200, &headers, black_box(&body), false, true).unwrap();
        assert!(response.as_ref().ends_with(&body));
        black_box(response);
      });
    });
    assert_eq!(budget.used(), 0);
  }
  group.finish();
}
criterion_group!(benches, http);
criterion_main!(benches);
