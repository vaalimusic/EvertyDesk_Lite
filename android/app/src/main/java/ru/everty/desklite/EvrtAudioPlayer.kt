package ru.everty.desklite

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack

/**
 * Низколатентный аудио-плеер для EVRT потока с adaptive jitter buffer.
 *
 * Алгоритм:
 *  1. Ждём AudioConfig от хоста (sample rate != 0), до 800 мс.
 *  2. Прогрев: накапливаем TARGET_FRAMES в очереди перед стартом воспроизведения.
 *     Это сглаживает джиттер первых пакетов без задержки в установившемся режиме.
 *  3. Steady-state: играем фрейм за фреймом.
 *     - Очередь > MAX_FRAMES: дропаем лишние (burst после паузы / Sleep).
 *     - Очередь пуста: sleep 5 мс (AudioTrack продолжает выводить из своего буфера).
 *  4. Мьют: дрейним очередь без записи в AudioTrack — при анмьюте нет задержки.
 */
class EvrtAudioPlayer(private val client: NativeClient) {

    private val channelMask = AudioFormat.CHANNEL_OUT_STEREO
    private val encoding   = AudioFormat.ENCODING_PCM_16BIT

    // Jitter buffer: накопить эти фреймы (×10 мс = 30 мс) перед стартом.
    private val TARGET_FRAMES = 3
    // Максимум в очереди: выше — дроп (800 мс при 256 макс. в Rust; здесь мягкий порог).
    private val MAX_FRAMES = 20  // 200 мс — ограничение сетевого burst

    @Volatile var muted = false
    @Volatile private var running = false
    private var thread: Thread? = null
    private var track: AudioTrack? = null

    private fun buildTrack(sampleRate: Int): AudioTrack {
        val minBuf = AudioTrack.getMinBufferSize(sampleRate, channelMask, encoding)
        val bufSize = maxOf(minBuf, 1920 * 4)
        return AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_GAME)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                    .build()
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setEncoding(encoding)
                    .setSampleRate(sampleRate)
                    .setChannelMask(channelMask)
                    .build()
            )
            .setBufferSizeInBytes(bufSize)
            .setTransferMode(AudioTrack.MODE_STREAM)
            .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
            .build()
    }

    fun start() {
        if (running) return
        running = true
        thread = Thread {
            // ── Шаг 1: ждём AudioConfig (rate != 0) до 800 мс ────────────────
            var waited = 0
            var sampleRate = client.audioSampleRate()
            while (sampleRate == 0 && waited < 800 && running) {
                Thread.sleep(20)
                waited += 20
                sampleRate = client.audioSampleRate()
            }
            if (sampleRate == 0) sampleRate = 48000

            val t = buildTrack(sampleRate)
            track = t
            t.play()

            // ── Шаг 2: прогрев — ждём TARGET_FRAMES фреймов в очереди ────────
            while (client.audioQueueDepth() < TARGET_FRAMES && running) {
                Thread.sleep(5)
            }

            // ── Шаг 3: steady-state loop ──────────────────────────────────────
            while (running) {
                // Мягкий overflow: дропаем лишние фреймы чтобы не копилось
                while (client.audioQueueDepth() > MAX_FRAMES) {
                    client.pollAudio()
                }

                val pcm = client.pollAudio()
                if (pcm != null && pcm.isNotEmpty()) {
                    if (!muted) {
                        t.write(pcm, 0, pcm.size)
                    }
                    // мьют: фрейм взят из очереди, очередь не пухнет
                } else {
                    // Очередь пуста — AudioTrack доигрывает свой буфер, ждём данных
                    Thread.sleep(5)
                }
            }
        }.also {
            it.name = "evrt-audio"
            it.priority = Thread.MAX_PRIORITY
            it.isDaemon = true
            it.start()
        }
    }

    fun stop() {
        running = false
        thread?.join(500)
        thread = null
        try {
            track?.stop()
            track?.release()
            track = null
        } catch (_: Exception) {}
    }
}
