package dev.elide.bemo.examples.ktor

import dev.elide.bemo.examples.NettyBaseline
import dev.elide.bemo.examples.BemoRuntime
import dev.elide.bemo.examples.BenchmarkCompression
import dev.elide.bemo.examples.BenchmarkPayload
import dev.elide.bemo.examples.BenchmarkTls
import dev.elide.bemo.transport.NativeIoHandler
import dev.elide.bemo.transport.NativeServerSocketChannel
import io.ktor.http.ContentType
import io.ktor.http.HttpStatusCode
import io.ktor.server.application.Application as KtorApplication
import io.ktor.server.engine.EngineConnectorBuilder
import io.ktor.server.engine.embeddedServer
import io.ktor.server.netty.Netty
import io.ktor.server.response.respondBytes
import io.ktor.server.routing.get
import io.ktor.server.routing.routing
import io.netty.channel.MultiThreadIoEventLoopGroup
import java.util.concurrent.CountDownLatch

/** Ktor keeps HTTP routing and codecs; its bootstrap hook selects Bemo's transport. */
object Application {
  @JvmStatic
  fun main(args: Array<String>) {
    val tls = BenchmarkTls()
    fun server(port: Int, secure: Boolean) = embeddedServer(Netty, configure = {
      connectors.add(EngineConnectorBuilder().apply {
        host = "127.0.0.1"
        this.port = port
      })
      enableHttp2 = false
      shareWorkGroup = true
      connectionGroupSize = 1
      workerGroupSize = 2
      callGroupSize = 1
      configureBootstrap = {
        if (BenchmarkPayload.bemoEnabled()) {
          val binding = BemoRuntime.create()
          val factory = NativeIoHandler.newFactory(binding, 0, 256, 64 * 1024 * 1024)
          group(MultiThreadIoEventLoopGroup(1, factory), MultiThreadIoEventLoopGroup(2, factory))
          channelFactory { NativeServerSocketChannel() }
        } else {
          group(
            MultiThreadIoEventLoopGroup(1, NettyBaseline.factory()),
            MultiThreadIoEventLoopGroup(2, NettyBaseline.factory())
          )
          channelFactory {
            NettyBaseline.channel(io.netty.channel.socket.ServerSocketChannel::class.java)
              .also { NettyBaseline.verifyChannel(it) }
          }
        }
      }
      if (secure) {
        val context = tls.context()
        channelPipelineConfig = { addFirst("bemo-tls", context.newHandler(channel().alloc())) }
      }
    }) { routes(secure) }

    val http = server(Integer.getInteger("server.port", 8080), false)
    val https = if (System.getProperty("bemo.tls.enabled", "true").toBoolean()) {
      server(Integer.getInteger("bemo.tls.port", 8443), true)
    } else null
    val stopped = CountDownLatch(1)
    Runtime.getRuntime().addShutdownHook(Thread {
      try {
        https?.stop(0, 5000)
        http.stop(0, 5000)
      } finally {
        tls.close()
        stopped.countDown()
      }
    })
    try {
      http.start(false)
      https?.start(false)
      stopped.await()
    } finally {
      https?.stop(0, 5000)
      http.stop(0, 5000)
      tls.close()
    }
  }

  private fun KtorApplication.routes(secure: Boolean) {
    routing {
      get("/plaintext") {
        call.respondBytes("Hello, World!".toByteArray(), ContentType.Text.Plain)
      }
      for (path in listOf("/payload", "/compression", "/tls", "/tls-compression")) {
        get(path) {
          if (path.startsWith("/tls") && !secure) {
            call.respondBytes(byteArrayOf(), ContentType.Text.Plain, HttpStatusCode.UpgradeRequired)
          } else {
            if (secure) call.response.headers.append("X-TLS-Provider", BenchmarkTls.provider())
            val compress = path.contains("compression")
            if (compress) call.response.headers.append("Vary", "Accept-Encoding")
            val gzip = compress && BenchmarkPayload.acceptsGzip(call.request.headers["Accept-Encoding"])
            if (gzip) {
              call.response.headers.append("Content-Encoding", "gzip")
              call.response.headers.append("X-Compression-Provider", BenchmarkCompression.provider())
            }
            call.respondBytes(if (gzip) BenchmarkCompression.gzip() else BenchmarkPayload.body(), ContentType.Text.Plain)
          }
        }
      }
    }
  }
}
