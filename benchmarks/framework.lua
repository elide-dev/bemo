-- Validate every completed reply; no pipelining or per-request Lua generation.
local threads = {}
local ffi = require("ffi")
local zlib
local stream
local decoded
local expected
local compressed
local compression_provider
local tls_provider

ffi.cdef[[
typedef struct {
  unsigned char *next_in; unsigned int avail_in; unsigned long total_in;
  unsigned char *next_out; unsigned int avail_out; unsigned long total_out;
  char *msg; void *state; void *zalloc; void *zfree; void *opaque;
  int data_type; unsigned long adler; unsigned long reserved;
} bemo_z_stream;
const char *zlibVersion(void);
int inflateInit2_(bemo_z_stream *, int, const char *, int);
int inflateReset(bemo_z_stream *);
int inflate(bemo_z_stream *, int);
int inflateEnd(bemo_z_stream *);
]]

function setup(thread)
  table.insert(threads, thread)
end

function init(args)
  invalid = 0
  wire_min = math.huge
  wire_max = 0
  local size = tonumber(args[1]) or 13
  compressed = args[2] == "gzip"
  compression_provider = args[3]
  tls_provider = args[4]
  if size == 13 then
    expected = "Hello, World!"
  else
    local pattern = "Hello, World! Bemo framework benchmark.\n"
    expected = string.rep(pattern, math.ceil(size / #pattern)):sub(1, size)
  end
  if compressed then
    zlib = ffi.load("z")
    stream = ffi.new("bemo_z_stream[1]")
    assert(zlib.inflateInit2_(stream, 31, zlib.zlibVersion(), ffi.sizeof("bemo_z_stream")) == 0)
    stream = ffi.gc(stream, function(value) zlib.inflateEnd(value) end)
    decoded = ffi.new("unsigned char[?]", size + 1)
  end
end

local function header(headers, name)
  for key, value in pairs(headers) do
    if string.lower(key) == name then return value end
  end
end

function response(status, headers, body)
  wire_min = math.min(wire_min, #body)
  wire_max = math.max(wire_max, #body)
  local valid = status == 200
  if compressed then
    valid = valid and header(headers, "content-encoding") == "gzip"
      and header(headers, "x-compression-provider") == compression_provider
      and header(headers, "vary") == "Accept-Encoding"
    assert(zlib.inflateReset(stream) == 0)
    stream[0].next_in = ffi.cast("unsigned char *", body)
    stream[0].avail_in = #body
    stream[0].next_out = decoded
    stream[0].avail_out = #expected + 1
    -- Z_STREAM_END checks the gzip trailer/CRC; reject extra output or wire bytes.
    local result = zlib.inflate(stream, 4)
    valid = valid and result == 1 and stream[0].avail_in == 0
      and stream[0].total_out == #expected
      and ffi.string(decoded, tonumber(stream[0].total_out)) == expected
  else
    valid = valid and header(headers, "content-encoding") == nil and body == expected
  end
  if tls_provider and tls_provider ~= "none" then
    valid = valid and header(headers, "x-tls-provider") == tls_provider
  end
  if not valid then
    invalid = invalid + 1
  end
end

function done(summary, latency, requests)
  local bad = 0
  local smallest = math.huge
  local largest = 0
  for _, thread in ipairs(threads) do
    bad = bad + thread:get("invalid")
    smallest = math.min(smallest, thread:get("wire_min"))
    largest = math.max(largest, thread:get("wire_max"))
  end
  io.write(string.format(
    '\nBEMO_RESULT {"wire_body_min_bytes":%d,"wire_body_max_bytes":%d,"requests":%d,"duration_us":%d,"requests_per_second":%.3f,"latency_p50_us":%.3f,"latency_p99_us":%.3f,"invalid_responses":%d,"connect_errors":%d,"read_errors":%d,"write_errors":%d,"status_errors":%d,"timeouts":%d}\n',
    smallest, largest, summary.requests, summary.duration, summary.requests * 1e6 / summary.duration,
    latency:percentile(50), latency:percentile(99), bad,
    summary.errors.connect, summary.errors.read, summary.errors.write,
    summary.errors.status, summary.errors.timeout))
end
