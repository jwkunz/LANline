package land.my.sdrc2

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
 * Listens for the SDR C2 UDP discovery beacon (see docs/beacon-protocol.md) and
 * keeps a short-TTL map of the servers currently announcing on the LAN.
 */
class BeaconListener(private val port: Int = 50055) {

    private data class Server(
        val serverId: String,
        val hostname: String,
        val baseUrl: String,
        val c2: Int,
        val audioOut: Int,
        val audioIn: Int,
        val device: String?,
        val lastSeenMs: Long,
    )

    private val servers = ConcurrentHashMap<String, Server>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var job: Job? = null
    private var socket: DatagramSocket? = null

    fun start() {
        if (job?.isActive == true) return
        job = scope.launch {
            try {
                val s = DatagramSocket(null).apply {
                    reuseAddress = true
                    broadcast = true
                    bind(InetSocketAddress(port))
                }
                socket = s
                val buf = ByteArray(8192)
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
            if (o.optString("magic") != "SDR-C2-BEACON") return
            if (o.optInt("protocol_version") != 1) return
            val ports = o.getJSONObject("ports")
            val id = o.getString("server_id")
            servers[id] = Server(
                serverId = id,
                hostname = o.optString("hostname", "?"),
                baseUrl = o.optString("c2_base_url"),
                c2 = ports.optInt("c2"),
                audioOut = ports.optInt("audio_out"),
                audioIn = ports.optInt("audio_in"),
                device = o.optJSONObject("device")?.optString("label"),
                lastSeenMs = System.currentTimeMillis(),
            )
        } catch (e: Exception) {
            Log.d(TAG, "bad beacon datagram: ${e.message}")
        }
    }

    private fun prune() {
        val cutoff = System.currentTimeMillis() - TTL_MS
        servers.entries.removeAll { it.value.lastSeenMs < cutoff }
    }

    /** JSON array of the servers seen recently, for the WebView bridge. */
    fun snapshotJson(): String {
        prune()
        val now = System.currentTimeMillis()
        val arr = JSONArray()
        servers.values.sortedBy { it.hostname }.forEach { srv ->
            arr.put(JSONObject().apply {
                put("server_id", srv.serverId)
                put("hostname", srv.hostname)
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

    companion object {
        private const val TAG = "SdrBeacon"
        private const val TTL_MS = 5_000L
    }
}
