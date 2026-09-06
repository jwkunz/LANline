package land.lanline

import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.util.concurrent.ConcurrentHashMap

/**
 * Listens for the LANline UDP discovery beacons (see docs/beacon-protocol.md):
 *  - `LANLINE-BEACON` — one per server; kept in a short-TTL map by `server_id`.
 *  - `LANLINE-FLEET-BEACON` — emitted by `lanline-hypervisor`, describing a
 *    group of co-hosted radios; kept by `fleet_id`.
 */
class BeaconListener(private val port: Int = 50055) {

    data class Server(
        val serverId: String,
        val hostname: String,
        val label: String?,
        val baseUrl: String,
        val c2: Int,
        val audioOut: Int,
        val audioIn: Int,
        val device: String?,
        val lastSeenMs: Long,
    )

    data class Radio(
        val idx: Int,
        val serverId: String,
        val label: String,
        val c2BaseUrl: String,
        val device: String?,
        val running: Boolean,
    )

    data class Fleet(
        val fleetId: String,
        val hostname: String,
        val fleetBaseUrl: String,
        val radios: List<Radio>,
        val lastSeenMs: Long,
    )

    private val servers = ConcurrentHashMap<String, Server>()
    private val fleets = ConcurrentHashMap<String, Fleet>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var job: Job? = null
    private var socket: DatagramSocket? = null

    fun start() {
        if (job?.isActive == true) return
        job = scope.launch {
            try {
                val s = DatagramSocket(null as java.net.SocketAddress?)
                s.reuseAddress = true
                s.bind(InetSocketAddress(port))
                runCatching { s.broadcast = true }
                socket = s
                Log.i(TAG, "listening on udp/$port")
                val buf = ByteArray(16384)
                while (isActive) {
                    val pkt = DatagramPacket(buf, buf.size)
                    try {
                        s.receive(pkt)
                    } catch (e: Exception) {
                        if (isActive) Log.w(TAG, "recv failed: ${e.message}")
                        break
                    }
                    parse(String(pkt.data, 0, pkt.length, Charsets.UTF_8))
                    prune()
                }
            } catch (e: Exception) {
                Log.e(TAG, "listener failed: ${e.message}")
            } finally {
                socket?.close()
                socket = null
            }
        }
    }

    fun stop() {
        job?.cancel()
        job = null
        socket?.close()
        socket = null
    }

    private fun parse(text: String) {
        try {
            val o = JSONObject(text)
            if (o.optInt("protocol_version") != 1) return
            when (o.optString("magic")) {
                "LANLINE-BEACON" -> parseServer(o)
                "LANLINE-FLEET-BEACON" -> parseFleet(o)
            }
        } catch (e: Exception) {
            Log.d(TAG, "bad beacon datagram: ${e.message}")
        }
    }

    private fun parseServer(o: JSONObject) {
        val ports = o.getJSONObject("ports")
        val id = o.getString("server_id")
        servers[id] = Server(
            serverId = id,
            hostname = o.optString("hostname", "?"),
            label = o.optStringOrNull("instance_label"),
            baseUrl = o.optString("c2_base_url"),
            c2 = ports.optInt("c2"),
            audioOut = ports.optInt("audio_out"),
            audioIn = ports.optInt("audio_in"),
            device = o.optJSONObject("device")?.optString("label"),
            lastSeenMs = System.currentTimeMillis(),
        )
    }

    private fun parseFleet(o: JSONObject) {
        val id = o.getString("fleet_id")
        val arr = o.optJSONArray("radios") ?: JSONArray()
        val radios = ArrayList<Radio>(arr.length())
        for (i in 0 until arr.length()) {
            val r = arr.getJSONObject(i)
            radios.add(
                Radio(
                    idx = r.optInt("idx", i),
                    serverId = r.optString("server_id"),
                    label = r.optString("label", "radio-${r.optInt("idx", i)}"),
                    c2BaseUrl = r.optString("c2_base_url"),
                    device = r.optStringOrNull("device"),
                    running = r.optBoolean("running", false),
                )
            )
        }
        fleets[id] = Fleet(
            fleetId = id,
            hostname = o.optString("hostname", "?"),
            fleetBaseUrl = o.optString("fleet_base_url"),
            radios = radios,
            lastSeenMs = System.currentTimeMillis(),
        )
    }

    private fun prune() {
        val cutoff = System.currentTimeMillis() - TTL_MS
        servers.entries.removeAll { it.value.lastSeenMs < cutoff }
        fleets.entries.removeAll { it.value.lastSeenMs < cutoff }
    }

    /** Fleets seen recently, newest activity first. */
    fun fleetSnapshot(): List<Fleet> {
        prune()
        return fleets.values.sortedBy { it.hostname }
    }

    /** Servers seen recently that are **not** part of any current fleet. */
    fun standaloneServers(): List<Server> {
        prune()
        val claimed = fleets.values.flatMap { f -> f.radios.map { it.serverId } }.toHashSet()
        return servers.values.filter { it.serverId !in claimed }.sortedBy { it.hostname }
    }

    /** JSON array of every server seen recently, for the WebView bridge. */
    fun snapshotJson(): String {
        prune()
        val now = System.currentTimeMillis()
        val arr = JSONArray()
        servers.values.sortedBy { it.hostname }.forEach { srv ->
            arr.put(JSONObject().apply {
                put("server_id", srv.serverId)
                put("hostname", srv.hostname)
                put("instance_label", srv.label ?: JSONObject.NULL)
                put("c2_base_url", srv.baseUrl)
                put("ports", JSONObject().apply {
                    put("c2", srv.c2)
                    put("audio_out", srv.audioOut)
                    put("audio_in", srv.audioIn)
                })
                put("device", srv.device ?: JSONObject.NULL)
                put("age_ms", now - srv.lastSeenMs)
            })
        }
        return arr.toString()
    }

    /** JSON array of fleets, for the WebView bridge (future tabbed UI). */
    fun fleetsJson(): String {
        val arr = JSONArray()
        fleetSnapshot().forEach { f ->
            arr.put(JSONObject().apply {
                put("fleet_id", f.fleetId)
                put("hostname", f.hostname)
                put("fleet_base_url", f.fleetBaseUrl)
                put("radios", JSONArray().apply {
                    f.radios.forEach { r ->
                        put(JSONObject().apply {
                            put("idx", r.idx)
                            put("server_id", r.serverId)
                            put("label", r.label)
                            put("c2_base_url", r.c2BaseUrl)
                            put("device", r.device ?: JSONObject.NULL)
                            put("running", r.running)
                        })
                    }
                })
            })
        }
        return arr.toString()
    }

    private fun JSONObject.optStringOrNull(key: String): String? =
        if (isNull(key) || !has(key)) null else optString(key).ifEmpty { null }

    companion object {
        private const val TAG = "SdrBeacon"
        private const val TTL_MS = 5_000L
    }
}
