const { strictEqual, deepStrictEqual, throws } = require("node:assert");
const initEvents = [];
const initialized = new Response(null, {
  headers: {
    get "X-Test"() {
      initEvents.push("read");
      return "before";
    },
  },
});
deepStrictEqual(initEvents, ["read"]);
const initializedClone = initialized.clone();
strictEqual(initialized.headers, initialized.headers);
initialized.headers.set("x-test", "after");
strictEqual(initializedClone.headers.get("x-test"), "before");
for (const response of [
  Response.error(),
  Response.redirect("https://example.com/"),
]) {
  const clone = response.clone();
  throws(() => response.headers.set("x-test", "bad"), TypeError);
  throws(() => clone.headers.set("x-test", "bad"), TypeError);
}
throws(() => new Response(null, { headers: { "bad:name": "x" } }), TypeError);

function literalResponse() {
  return new Response("literal", { headers: { "X-Literal": "café" } });
}
for (let i = 0; i < 10; i++) literalResponse();
function dynamicResponse(value) {
  return new Response("dynamic", {
    headers: { "X-Fixed": "café", "X-Dynamic": value },
  });
}
for (let i = 0; i < 10; i++) dynamicResponse("seed");
let checkpoint = 0;
let retained = "";
const server = Elide.serve({
  port: 0,
  fetch(request) {
    const url = new URL(request.url);
    switch (url.pathname) {
      case "/dynamic":
        return dynamicResponse(
          "é".repeat(Number(url.searchParams.get("size"))),
        );
      case "/stream": {
        let remaining = 96;
        return new Response(
          new ReadableStream({
            pull(controller) {
              if (remaining-- > 0)
                controller.enqueue(new Uint8Array(4096).fill(120));
              else controller.close();
            },
          }),
        );
      }
      case "/tick":
        process.nextTick(() => {
          checkpoint = 1;
        });
        Promise.resolve().then(() => {
          checkpoint *= 2;
        });
        return new Response("tick");
      case "/check":
        return new Response(String(checkpoint));
      case "/literal": {
        const original = literalResponse();
        const clone = original.clone();
        original.headers.set("x-literal", "mutated");
        process.nextTick(() => clone.headers.set("x-literal", "late"));
        return clone;
      }
      case "/thenable":
        return {
          then(resolve, reject) {
            resolve(new Response("first"));
            reject(new Error("late rejection"));
            resolve(new Response("second"));
          },
        };
      case "/retain":
        process.nextTick(() => {
          retained =
            new URL(request.url).pathname + ":" + request.headers.get("host");
        });
        return new Response("retained");
      case "/retained":
        return new Response(retained);
      case "/async":
        return new Promise((resolve) => {
          setTimeout(() => resolve(new Response("async")), 20);
        });
      case "/headers": {
        const original = new Response(
          new Uint8Array([101, 110, 99, 111, 100, 101, 100]),
          {
            headers: [
              ["X-Test", "before"],
              ["Set-Cookie", "a=1"],
              ["Set-Cookie", "b=2"],
              ["X-Latin", "café"],
              ["Content-Length", "9000"],
            ],
          },
        );
        const clone = original.clone();
        original.headers.set("x-test", "original-mutated");
        process.nextTick(() => clone.headers.set("x-test", "late"));
        return clone;
      }
      case "/json":
        return Response.json({ ok: true });
      case "/head":
        return new Response("not-on-wire");
      case "/204":
        return new Response(null, { status: 204 });
      case "/205":
        return new Response(null, { status: 205 });
      case "/304":
        return new Response(null, { status: 304 });
      case "/throw":
        throw new Error("callback smoke failure");
      case "/fatal":
        setTimeout(() => console.log("UNEXPECTED_TIMER_AFTER_FATAL"), 0);
        process.nextTick(() => {
          throw new Error("fatal callback smoke");
        });
        return new Response("fatal");
      case "/marker":
        console.log("UNEXPECTED_REQUEST_AFTER_FATAL");
        return new Response("marker");
      case "/close":
        server.close();
        return new Response("closed");
      default:
        return new Response("callback");
    }
  },
});
console.log(`CALLBACK_PORT=${server.port}`);
