package dev.godscamera.android

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executor
import java.util.concurrent.Executors

/** A Mac running God's Camera, as it announces itself on the local network. */
data class FoundMac(
    /** The Mac's name, as its owner gave it. */
    val name: String,
    /** Where it listens, as host:port, the most usable address first. */
    val addresses: List<String>,
    /** The first digits of its certificate's fingerprint, when it announces them. */
    val fingerprintStart: String?,
) {
    /** Whether this could be the Mac with certificate [fingerprint]. The certificate itself decides in the end. */
    fun couldBe(fingerprint: String): Boolean =
        fingerprintStart != null && fingerprintStart.length >= 8 && fingerprint.startsWith(fingerprintStart.lowercase())

    /**
     * Whether this is worth trying as [mac]: it announces the start of the
     * same fingerprint, or it announces none and goes by the same name.
     * Trying costs nothing: a Mac with another certificate is not talked to.
     */
    fun mayBe(mac: PairedMac): Boolean = couldBe(mac.fingerprint) || (fingerprintStart == null && name == mac.name)
}

/** The addresses of a service in the order they should be tried, as host:port. */
fun usableAddresses(hosts: List<InetAddress>, port: Int): List<String> {
    if (port !in 1..65535) return emptyList()
    val v4 = hosts.filterIsInstance<Inet4Address>().mapNotNull { it.hostAddress }.map { "$it:$port" }
    // An address that is only good on one network interface cannot be put in a URL.
    val v6 = hosts.filterIsInstance<Inet6Address>()
        .filter { !it.isLinkLocalAddress && !it.isLoopbackAddress }
        .mapNotNull { it.hostAddress?.substringBefore('%') }
        .map { "[$it]:$port" }
    return (v4 + v6).distinct()
}

/**
 * Looks for Macs on the local network. A Mac running God's Camera announces
 * itself the way printers do (Bonjour), so nobody has to type an address.
 */
class MacFinder(context: Context) {
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val executor: Executor = Executors.newSingleThreadExecutor { Thread(it, "gc-find").apply { isDaemon = true } }
    private val services = ConcurrentHashMap<String, FoundMac>()
    private val watches = ConcurrentHashMap<String, NsdManager.ServiceInfoCallback>()
    private val _found = MutableStateFlow<List<FoundMac>>(emptyList())

    /** The Macs in sight, by name. It changes as Macs come and go. */
    val found: StateFlow<List<FoundMac>> = _found
    private var listener: NsdManager.DiscoveryListener? = null

    private fun publish() {
        _found.value = services.values.sortedBy { it.name.lowercase() }
    }

    @Synchronized
    fun start() {
        if (listener != null || nsd == null) return
        val created = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {}
            override fun onDiscoveryStopped(serviceType: String) {}
            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                synchronized(this@MacFinder) { if (listener === this) listener = null }
            }
            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
            override fun onServiceFound(service: NsdServiceInfo) = watch(service)
            override fun onServiceLost(service: NsdServiceInfo) = forget(service.serviceName)
        }
        listener = created
        try {
            nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, created)
        } catch (_: RuntimeException) {
            listener = null
        }
    }

    @Synchronized
    fun stop() {
        val current = listener ?: return
        listener = null
        runCatching { nsd?.stopServiceDiscovery(current) }
        watches.values.forEach { watch -> runCatching { nsd?.unregisterServiceInfoCallback(watch) } }
        watches.clear()
        services.clear()
        publish()
    }

    /** Follow one announced service: its addresses arrive, and change, through the callback. */
    private fun watch(service: NsdServiceInfo) {
        val name = service.serviceName ?: return
        if (watches.containsKey(name)) return
        val callback = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                watches.remove(name)
            }
            override fun onServiceUpdated(info: NsdServiceInfo) {
                val text = info.attributes.mapValues { (_, value) -> value?.toString(Charsets.UTF_8).orEmpty() }
                val addresses = usableAddresses(info.hostAddresses, info.port)
                if (addresses.isEmpty()) return
                services[name] = FoundMac(
                    name = text["name"]?.takeIf { it.isNotBlank() } ?: name,
                    addresses = addresses,
                    fingerprintStart = text["fp"]?.takeIf { it.isNotBlank() },
                )
                publish()
            }
            override fun onServiceLost() = forget(name)
            override fun onServiceInfoCallbackUnregistered() {}
        }
        watches[name] = callback
        try {
            nsd?.registerServiceInfoCallback(service, executor, callback)
        } catch (_: RuntimeException) {
            watches.remove(name)
        }
    }

    private fun forget(name: String?) {
        name ?: return
        watches.remove(name)?.let { watch -> runCatching { nsd?.unregisterServiceInfoCallback(watch) } }
        if (services.remove(name) != null) publish()
    }

    companion object {
        /** How a Mac running God's Camera announces its scanner. */
        const val SERVICE_TYPE = "_godscamera._tcp"
    }
}
