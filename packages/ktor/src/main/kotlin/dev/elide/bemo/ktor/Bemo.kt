/*
 * Copyright (c) 2026 Elide Technologies, Inc.
 * SPDX-License-Identifier: Apache-2.0
 */
@file:OptIn(io.ktor.utils.io.InternalAPI::class)

package dev.elide.bemo.ktor

import dev.elide.bemo.transport.TransportNative
import dev.elide.bemo.transport.http.NativeHttpServer
import io.ktor.events.Events
import io.ktor.http.*
import io.ktor.http.content.OutgoingContent
import io.ktor.server.application.*
import io.ktor.server.engine.*
import io.ktor.server.request.RequestCookies
import io.ktor.server.response.ResponseHeaders
import io.ktor.utils.io.*
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executor
import java.util.concurrent.Flow
import kotlin.coroutines.CoroutineContext
import kotlinx.coroutines.*
import kotlinx.coroutines.future.await

/** Native HTTP/1.1 engine; application routing and transformations remain Ktor's. */
object Bemo : ApplicationEngineFactory<BemoEngine, BemoEngine.Configuration> {
  override fun configuration(configure: BemoEngine.Configuration.() -> Unit) =
      BemoEngine.Configuration().apply(configure)

  override fun create(
      environment: ApplicationEnvironment,
      monitor: Events,
      developmentMode: Boolean,
      configuration: BemoEngine.Configuration,
      applicationProvider: () -> Application,
  ) = BemoEngine(environment, monitor, developmentMode, configuration, applicationProvider)
}

class BemoEngine(
    environment: ApplicationEnvironment,
    monitor: Events,
    developmentMode: Boolean,
    private val configuration: Configuration,
    private val applicationProvider: () -> Application,
) : BaseApplicationEngine(environment, monitor, developmentMode) {
  class Configuration : BaseApplicationEngine.Configuration() {
    var nativeMemoryLimit: Long = 256L * 1024 * 1024
    var requestBodyLimit: Long = 16L * 1024 * 1024
    var exchangeLimit: Int = 8192
    var nativeWorkers: Int = 1
    var dispatchCallsOnNativeWorker: Boolean = false
    var certificatePem: ByteArray? = null
    var privateKeyPem: ByteArray? = null
  }

  private val servers = mutableListOf<NativeHttpServer>()
  private val workerDispatchers = ConcurrentHashMap<Executor, CoroutineDispatcher>()

  override fun start(wait: Boolean): BemoEngine {
    val api =
        TransportNative.load() ?: error("Bemo backend unavailable; select the stock Ktor engine")
    val resolved = mutableListOf<EngineConnectorConfig>()
    try {
      for (connector in configuration.connectors) {
        val server =
            NativeHttpServer(
                api,
                connector.host,
                connector.port,
                configuration.nativeMemoryLimit,
                configuration.requestBodyLimit,
                configuration.exchangeLimit,
                { request -> handle(request, connector) },
                configuration.certificatePem,
                configuration.privateKeyPem,
                configuration.nativeWorkers,
            )
        servers.add(server)
        val port = server.start().toCompletableFuture().join()
        resolved.add(connector.withPort(port))
        println(
            "Bemo native HTTP: backend=${api.javaClass.name} port=$port tls=${configuration.certificatePem != null}"
        )
      }
      resolvedConnectorsDeferred.complete(resolved)
      monitor.raise(ServerReady, environment)
      if (wait) for (server in servers) server.stopped().toCompletableFuture().join()
      return this
    } catch (failure: Throwable) {
      stop(0, 0)
      throw failure
    }
  }

  private fun handle(
      input: NativeHttpServer.Request,
      connector: EngineConnectorConfig,
  ): CompletableFuture<NativeHttpServer.Response> {
    val application = applicationProvider()
    val ready = CompletableFuture<NativeHttpServer.Response>()
    val dispatcher =
        if (configuration.dispatchCallsOnNativeWorker)
            workerDispatchers.computeIfAbsent(input.executor()) { it.asCoroutineDispatcher() }
        else Dispatchers.Default
    val context =
        application.coroutineContext + SupervisorJob(application.coroutineContext[Job]) + dispatcher
    val call = BemoCall(application, context, input, connector, ready)
    val task =
        CoroutineScope(context).launch {
          try {
            pipeline.execute(call, Unit)
            call.response.finish()
          } catch (failure: Throwable) {
            call.response.fail(failure)
          } finally {
            call.close()
            context[Job]?.cancel()
          }
        }
    ready.whenComplete { response, failure ->
      if (failure != null) task.cancel()
      else response.sent().whenComplete { _, error -> if (error != null) task.cancel() }
    }
    return ready
  }

  override fun stop(gracePeriodMillis: Long, timeoutMillis: Long) {
    monitor.raise(ApplicationStopPreparing, environment)
    for (server in servers) {
      server.close()
      server.stopped().toCompletableFuture().join()
    }
    servers.clear()
    workerDispatchers.clear()
  }
}

private class BemoCall(
    application: Application,
    override val coroutineContext: CoroutineContext,
    input: NativeHttpServer.Request,
    connector: EngineConnectorConfig,
    ready: CompletableFuture<NativeHttpServer.Response>,
) : BaseApplicationCall(application) {
  override val request = BemoRequest(this, input, connector)
  override val response = BemoResponse(this, ready)

  init {
    putResponseAttribute()
  }

  fun close() {
    request.close()
  }
}

private class BemoRequest(
    private val nativeCall: BemoCall,
    private val input: NativeHttpServer.Request,
    connector: EngineConnectorConfig,
) : BaseApplicationRequest(nativeCall) {
  override val engineHeaders = Headers.build {
    for (header in input.headers()) append(header.name(), header.value())
  }
  private val channel = ByteChannel(autoFlush = true)
  private var subscription: Flow.Subscription? = null
  override val engineReceiveChannel: ByteReadChannel = channel
  override val queryParameters: Parameters =
      parseQueryString(input.target().substringAfter('?', ""))
  override val rawQueryParameters: Parameters =
      parseQueryString(input.target().substringAfter('?', ""), decode = false)
  override val cookies = RequestCookies(this)
  override val local: RequestConnectionPoint =
      object : RequestConnectionPoint {
        override val scheme = if (input.secure()) "https" else "http"
        override val version = input.version()
        override val uri = input.target()
        override val method = HttpMethod.parse(input.method())
        override val localPort = input.localAddress().port
        override val serverPort =
            engineHeaders[HttpHeaders.Host]?.substringAfterLast(':', "")?.toIntOrNull() ?: localPort
        override val localHost = input.localAddress().hostString
        override val serverHost = engineHeaders[HttpHeaders.Host]?.substringBefore(':') ?: localHost
        override val localAddress = input.localAddress().address.hostAddress
        override val remoteHost = input.remoteAddress().hostString
        override val remoteAddress = input.remoteAddress().address.hostAddress
        override val remotePort = input.remoteAddress().port
        @Suppress("OVERRIDE_DEPRECATION") override val host = serverHost
        @Suppress("OVERRIDE_DEPRECATION") override val port = serverPort
      }

  init {
    input
        .body()
        .subscribe(
            object : Flow.Subscriber<ByteArray> {
              override fun onSubscribe(next: Flow.Subscription) {
                subscription = next
                next.request(1)
              }

              override fun onNext(bytes: ByteArray) {
                CoroutineScope(nativeCall.coroutineContext).launch(
                    start = CoroutineStart.UNDISPATCHED
                ) {
                  try {
                    channel.writeFully(bytes)
                    channel.flush()
                    subscription?.request(1)
                  } catch (failure: Throwable) {
                    subscription?.cancel()
                    channel.close(failure)
                  }
                }
              }

              override fun onError(failure: Throwable) {
                channel.close(failure)
              }

              override fun onComplete() {
                channel.close()
              }
            }
        )
  }

  fun close() {
    subscription?.cancel()
    if (!channel.isClosedForWrite) channel.cancel()
  }
}

private class BemoResponse(
    private val nativeCall: BemoCall,
    private val ready: CompletableFuture<NativeHttpServer.Response>,
) : BaseApplicationResponse(nativeCall) {
  private val values = HeadersBuilder()
  private var nativeResponse: NativeHttpServer.Response? = null
  private var output: ByteChannel? = null
  override val headers =
      object : ResponseHeaders() {
        override fun engineAppendHeader(name: String, value: String) {
          values.append(name, value)
        }

        override fun getEngineHeaderNames() = values.names().toList()

        override fun getEngineHeaderValues(name: String) = values.getAll(name).orEmpty()
      }

  override fun setStatus(statusCode: HttpStatusCode) {}

  private fun send(body: Flow.Publisher<ByteArray>, length: Long) {
    val responseHeaders = mutableListOf<NativeHttpServer.Header>()
    values.build().forEach { name, fields ->
      if (
          !name.equals(HttpHeaders.ContentLength, true) &&
              !name.equals(HttpHeaders.TransferEncoding, true)
      ) {
        for (value in fields) responseHeaders.add(NativeHttpServer.Header(name, value))
      }
    }
    val response = NativeHttpServer.Response(status()?.value ?: 200, responseHeaders, length, body)
    nativeResponse = response
    check(ready.complete(response)) { "HTTP response already selected" }
  }

  override suspend fun respondFromBytes(bytes: ByteArray) {
    send(NativeHttpServer.fixedBody(bytes), bytes.size.toLong())
    nativeResponse!!.sent().await()
  }

  override suspend fun respondNoContent(content: OutgoingContent.NoContent) {
    send(NativeHttpServer.fixedBody(ByteArray(0)), 0)
    nativeResponse!!.sent().await()
  }

  override suspend fun responseChannel(): ByteWriteChannel {
    val channel = ByteChannel(autoFlush = true)
    output = channel
    val publisher =
        Flow.Publisher<ByteArray> { subscriber ->
          subscriber.onSubscribe(
              object : Flow.Subscription {
                private var cancelled = false

                override fun request(n: Long) {
                  if (cancelled) return
                  if (n != 1L) {
                    subscriber.onError(
                        IllegalArgumentException("one native response credit required")
                    )
                    return
                  }
                  CoroutineScope(nativeCall.coroutineContext).launch {
                    try {
                      val bytes = ByteArray(8192)
                      val count = channel.readAvailable(bytes)
                      if (count < 0) subscriber.onComplete()
                      else subscriber.onNext(bytes.copyOf(count))
                    } catch (failure: Throwable) {
                      subscriber.onError(failure)
                    }
                  }
                }

                override fun cancel() {
                  cancelled = true
                  if (!channel.isClosedForWrite) channel.cancel()
                }
              }
          )
        }
    send(publisher, values[HttpHeaders.ContentLength]?.toLongOrNull() ?: -1)
    return channel
  }

  override suspend fun respondUpgrade(upgrade: OutgoingContent.ProtocolUpgrade) {
    throw UnsupportedOperationException("HTTP upgrades require the stock Ktor engine")
  }

  suspend fun finish() {
    output?.flushAndClose()
    nativeResponse?.sent()?.await()
  }

  fun fail(failure: Throwable) {
    if (!ready.completeExceptionally(failure)) nativeResponse?.cancel(failure)
    output?.close(failure)
  }
}
