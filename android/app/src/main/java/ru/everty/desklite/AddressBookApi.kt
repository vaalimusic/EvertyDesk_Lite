package ru.everty.desklite

import org.json.JSONArray
import org.json.JSONObject
import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

data class AddressBookContact(
    val name: String,
    val remoteId: String,
    val note: String,
    val os: String,
    val online: Boolean,
)

class AddressBookApi(private val apiUrl: String) {
    fun login(username: String, password: String, localId: String, uuid: String): String {
        val body = JSONObject()
            .put("username", username)
            .put("password", password)
            .put("id", normalizeRemoteId(localId))
            .put("uuid", uuid)
            .put("autoLogin", true)
            .put("type", "account")
            .put(
                "deviceInfo",
                JSONObject()
                    .put("os", "Android")
                    .put("type", "Phone")
                    .put("name", android.os.Build.MODEL ?: "Android"),
            )

        val json = sendJson("POST", "/api/login", null, body)
        checkJsonError(json)
        return extractString(json, "access_token")
            ?.takeIf { it.isNotBlank() }
            ?: throw IllegalStateException(
                extractString(json, "type")?.let { "Login requires extra step: $it" }
                    ?: "API did not return access_token"
            )
    }

    fun personalAddressBookGuid(token: String): String {
        val json = sendJson("POST", "/api/ab/personal", token, JSONObject())
        checkJsonError(json)
        return extractString(json, "guid")
            ?: extractString(json, "id")
            ?: throw IllegalStateException("API did not return address book GUID")
    }

    fun peers(token: String, guid: String): List<AddressBookContact> {
        val contacts = mutableListOf<AddressBookContact>()
        var current = 1
        while (true) {
            val json = sendJson(
                "POST",
                "/api/ab/peers",
                token,
                JSONObject(),
                mapOf("ab" to guid, "pageSize" to "30", "current" to current.toString()),
            )
            checkJsonError(json)

            val data = json.optJSONArray("data")
                ?: JSONArray().also { fallback ->
                    if (json.has("id")) fallback.put(json)
                }

            for (i in 0 until data.length()) {
                val peer = data.optJSONObject(i) ?: continue
                val remoteId = extractString(peer, "id")?.let(::normalizeRemoteId) ?: continue
                contacts += AddressBookContact(
                    name = extractString(peer, "alias").orEmpty(),
                    remoteId = remoteId,
                    note = extractString(peer, "hostname").orEmpty(),
                    os = extractString(peer, "platform").orEmpty(),
                    online = peer.optBoolean("online", false),
                )
            }

            val total = json.optLong("total", contacts.size.toLong())
            if (data.length() < 30 || current * 30L >= total) break
            current += 1
        }
        return contacts
    }

    private fun sendJson(
        method: String,
        path: String,
        token: String?,
        body: JSONObject,
        query: Map<String, String> = emptyMap(),
    ): JSONObject {
        val url = URL(buildUrl(path, query))
        val conn = (url.openConnection() as HttpURLConnection).apply {
            requestMethod = method
            connectTimeout = 12_000
            readTimeout = 12_000
            doInput = true
            doOutput = true
            setRequestProperty("Content-Type", "application/json; charset=utf-8")
            token?.takeIf { it.isNotBlank() }?.let {
                setRequestProperty("Authorization", "Bearer $it")
            }
        }

        conn.outputStream.use { out ->
            out.write(body.toString().toByteArray(Charsets.UTF_8))
        }

        val code = conn.responseCode
        val stream = if (code in 200..299) conn.inputStream else conn.errorStream
        val text = stream?.use { input ->
            BufferedReader(InputStreamReader(input, Charsets.UTF_8)).readText()
        }.orEmpty()

        if (code !in 200..299) {
            throw IllegalStateException("API HTTP $code: $text")
        }
        if (text.isBlank()) return JSONObject().put("ok", true)
        return JSONObject(text)
    }

    private fun buildUrl(path: String, query: Map<String, String>): String {
        val base = apiUrl.trimEnd('/')
        if (query.isEmpty()) return "$base$path"
        val encoded = query.entries.joinToString("&") { (key, value) ->
            "${urlEncode(key)}=${urlEncode(value)}"
        }
        return "$base$path?$encoded"
    }

    private fun checkJsonError(json: JSONObject) {
        val error = extractString(json, "error")
        if (!error.isNullOrBlank()) throw IllegalStateException(error)
        val message = extractString(json, "message")
        if (!message.isNullOrBlank() && message != "ok") {
            throw IllegalStateException(message)
        }
    }

    private fun extractString(json: JSONObject, field: String): String? {
        if (json.has(field) && !json.isNull(field)) {
            val value = json.opt(field)
            if (value != null) return value.toString()
        }
        val data = json.optJSONObject("data")
        return data?.let { extractString(it, field) }
    }

    private fun normalizeRemoteId(value: String): String =
        value.filter { it.isDigit() }

    private fun urlEncode(value: String): String =
        URLEncoder.encode(value, "UTF-8")
}
