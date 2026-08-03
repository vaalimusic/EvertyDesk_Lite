package ru.everty.desklite

import android.content.Context
import android.graphics.SurfaceTexture
import android.opengl.GLES11Ext
import android.opengl.GLES20
import android.opengl.GLSurfaceView
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.Surface
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.opengles.GL10

/**
 * ROADMAP.md task #30 — replaces `TextureView`'s own internal SurfaceTexture
 * management for the MediaCodec video-decode output surface.
 *
 * Live-found on real hardware (MI_8/Adreno630, H265): under sparse/irregular
 * frame delivery — the natural cadence once DXGI's change-driven screen
 * capture on the host has nothing new to send (a static desktop produces a
 * new frame only every few real seconds, not a steady 30-60Hz) — the
 * `TextureView`-backed decode path froze visually, even though a live frame
 * counter proved `MediaCodec.Callback.onOutputBufferAvailable` kept firing
 * continuously and `releaseOutputBuffer(index, true)` kept succeeding the
 * entire time. Two other real hypotheses were tested and ruled out first:
 * a wedged hardware decoder (a watchdog was built for this — see
 * `VideoDecoder`'s own doc comment — but it never had anything to catch,
 * since decode never actually stalled) and an unusual near-all-keyframe
 * bitstream shape under sparse capture (fixed the periodic-keyframe trigger
 * to count real sent frames instead of wall-clock time — the freeze
 * persisted regardless). What's left, and what this class exists to test:
 * `TextureView`'s own internal `SurfaceTexture`/frame-availability handling
 * not reliably redrawing when frames arrive very sparsely or unevenly
 * spaced, unlike the continuous near-constant-rate video/camera preview it
 * was designed around.
 *
 * This manages the `SurfaceTexture` and its GL (OES external) texture
 * directly instead of delegating to `TextureView`'s internal one: our own
 * `SurfaceTexture.OnFrameAvailableListener` unconditionally requests a GL
 * render on every single arriving frame, however far apart in time — no
 * dependency on whatever internal cadence assumption `TextureView` makes.
 */
class VideoGLSurfaceView(context: Context) : GLSurfaceView(context) {

    /** Fires once the GL texture + `SurfaceTexture` are ready, on the main thread. */
    var onSurfaceReady: ((Surface) -> Unit)? = null

    /** Fires when this view is detached (screen torn down), on the main thread. */
    var onSurfaceGone: (() -> Unit)? = null

    private val glRenderer = Renderer()

    init {
        setEGLContextClientVersion(2)
        setRenderer(glRenderer)
        // Originally RENDERMODE_WHEN_DIRTY — redraw only exactly when a new
        // decoded frame arrived — deliberately, to rule out a TextureView
        // failure mode under investigation at the time (a steady internal
        // redraw cadence papering over irregular frame arrival). That
        // investigation is long closed (the real bug was the codec-race
        // overlay corrupting frames, not a rendering cadence issue — see
        // ROADMAP.md Phase 6.7/6.8) and living with WHEN_DIRTY since has a
        // real cost: it ties PRESENTATION timing directly to ARRIVAL
        // timing, so any network jitter (live-measured jitter_p95 ~15ms —
        // not negligible against a ~16.7ms/60fps frame budget) shows up
        // directly as uneven on-screen motion, even while the aggregate
        // decoded-fps counter reads a perfectly healthy 60. Live-reported:
        // "по счётчику 60 но плавности нет" — exactly this symptom.
        // CONTINUOUSLY decouples the two, the same way Moonlight/Sunshine-
        // class low-latency video presentation does: redraw on every
        // vsync tick regardless of arrival timing, showing whatever the
        // latest decoded frame is (`onDrawFrame` below already only calls
        // `updateTexImage()` when `frameAvailable` is actually set, so a
        // vsync tick with nothing new just redraws the same last frame,
        // not wasted decode work) — real motion smoothness now tracks the
        // DISPLAY's own steady clock, not the network's.
        // A/B tested against WHEN_DIRTY while chasing "телепорты" (a brief
        // flash back to an older-looking frame) — live-confirmed the
        // teleport happens in BOTH render modes, ruling this render-mode
        // choice out as the cause. Back to CONTINUOUSLY, which is a real,
        // separately-confirmed smoothness fix (see the paragraph above) —
        // see ROADMAP.md / task tracker for the still-open teleport
        // investigation.
        renderMode = RENDERMODE_CONTINUOUSLY
    }

    override fun onDetachedFromWindow() {
        onSurfaceGone?.invoke()
        val renderer = glRenderer
        queueEvent { renderer.release() }
        super.onDetachedFromWindow()
    }

    private inner class Renderer : GLSurfaceView.Renderer {
        private var programId = 0
        private var oesTextureId = 0
        private var surfaceTexture: SurfaceTexture? = null
        private val transformMatrix = FloatArray(16)
        private val frameLock = Any()
        private var frameAvailable = false

        private var aPositionLoc = 0
        private var aTexCoordLoc = 0
        private var uTexMatrixLoc = 0
        private var uTextureLoc = 0

        override fun onSurfaceCreated(gl: GL10?, config: EGLConfig?) {
            programId = buildProgram(VERTEX_SHADER, FRAGMENT_SHADER)
            aPositionLoc = GLES20.glGetAttribLocation(programId, "aPosition")
            aTexCoordLoc = GLES20.glGetAttribLocation(programId, "aTexCoord")
            uTexMatrixLoc = GLES20.glGetUniformLocation(programId, "uTexMatrix")
            uTextureLoc = GLES20.glGetUniformLocation(programId, "uTexture")

            val textures = IntArray(1)
            GLES20.glGenTextures(1, textures, 0)
            oesTextureId = textures[0]
            GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, oesTextureId)
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR)
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR)
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE)
            GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE)

            val st = SurfaceTexture(oesTextureId)
            st.setOnFrameAvailableListener {
                synchronized(frameLock) { frameAvailable = true }
                requestRender()
            }
            surfaceTexture = st

            val surface = Surface(st)
            mainHandler.post { onSurfaceReady?.invoke(surface) }
            Log.i(TAG, "GL surface created, OES texture=$oesTextureId")
        }

        override fun onSurfaceChanged(gl: GL10?, width: Int, height: Int) {
            GLES20.glViewport(0, 0, width, height)
        }

        override fun onDrawFrame(gl: GL10?) {
            val st = surfaceTexture ?: return
            val hadFrame = synchronized(frameLock) {
                val had = frameAvailable
                frameAvailable = false
                had
            }
            if (hadFrame) {
                try {
                    st.updateTexImage()
                    st.getTransformMatrix(transformMatrix)
                } catch (e: Exception) {
                    Log.e(TAG, "updateTexImage failed: $e")
                    return
                }
            }

            GLES20.glClearColor(0f, 0f, 0f, 1f)
            GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT)
            GLES20.glUseProgram(programId)

            GLES20.glActiveTexture(GLES20.GL_TEXTURE0)
            GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, oesTextureId)
            GLES20.glUniform1i(uTextureLoc, 0)
            GLES20.glUniformMatrix4fv(uTexMatrixLoc, 1, false, transformMatrix, 0)

            POSITION_BUFFER.position(0)
            GLES20.glVertexAttribPointer(aPositionLoc, 2, GLES20.GL_FLOAT, false, 0, POSITION_BUFFER)
            GLES20.glEnableVertexAttribArray(aPositionLoc)

            TEXCOORD_BUFFER.position(0)
            GLES20.glVertexAttribPointer(aTexCoordLoc, 4, GLES20.GL_FLOAT, false, 0, TEXCOORD_BUFFER)
            GLES20.glEnableVertexAttribArray(aTexCoordLoc)

            GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4)

            GLES20.glDisableVertexAttribArray(aPositionLoc)
            GLES20.glDisableVertexAttribArray(aTexCoordLoc)
        }

        /** Must run on the GL thread — call via `queueEvent`. */
        fun release() {
            surfaceTexture?.release()
            surfaceTexture = null
            if (oesTextureId != 0) {
                GLES20.glDeleteTextures(1, intArrayOf(oesTextureId), 0)
                oesTextureId = 0
            }
            if (programId != 0) {
                GLES20.glDeleteProgram(programId)
                programId = 0
            }
        }
    }

    companion object {
        private const val TAG = "EvdGLVideoView"
        private val mainHandler = Handler(Looper.getMainLooper())

        private const val VERTEX_SHADER = """
            attribute vec4 aPosition;
            attribute vec4 aTexCoord;
            uniform mat4 uTexMatrix;
            varying vec2 vTexCoord;
            void main() {
                gl_Position = aPosition;
                vTexCoord = (uTexMatrix * aTexCoord).xy;
            }
        """

        private const val FRAGMENT_SHADER = """
            #extension GL_OES_EGL_image_external : require
            precision mediump float;
            varying vec2 vTexCoord;
            uniform samplerExternalOES uTexture;
            void main() {
                gl_FragColor = texture2D(uTexture, vTexCoord);
            }
        """

        // Full-screen quad (triangle strip, clip-space -1..1) and matching
        // texture coordinates (as vec4 so they can be multiplied by
        // SurfaceTexture's 4x4 transform matrix — the correct way to handle
        // orientation/flip, instead of hardcoding a V-flip that may not
        // match every device's buffer layout).
        private val POSITION_BUFFER: FloatBuffer = floatBufferOf(
            -1f, -1f,
            1f, -1f,
            -1f, 1f,
            1f, 1f,
        )
        private val TEXCOORD_BUFFER: FloatBuffer = floatBufferOf(
            0f, 0f, 0f, 1f,
            1f, 0f, 0f, 1f,
            0f, 1f, 0f, 1f,
            1f, 1f, 0f, 1f,
        )

        private fun floatBufferOf(vararg values: Float): FloatBuffer =
            ByteBuffer.allocateDirect(values.size * 4)
                .order(ByteOrder.nativeOrder())
                .asFloatBuffer()
                .apply { put(values); position(0) }

        private fun compileShader(type: Int, source: String): Int {
            val shader = GLES20.glCreateShader(type)
            GLES20.glShaderSource(shader, source)
            GLES20.glCompileShader(shader)
            val status = IntArray(1)
            GLES20.glGetShaderiv(shader, GLES20.GL_COMPILE_STATUS, status, 0)
            if (status[0] == 0) {
                val log = GLES20.glGetShaderInfoLog(shader)
                GLES20.glDeleteShader(shader)
                throw RuntimeException("Shader compile failed: $log")
            }
            return shader
        }

        private fun buildProgram(vertexSrc: String, fragmentSrc: String): Int {
            val vertexShader = compileShader(GLES20.GL_VERTEX_SHADER, vertexSrc)
            val fragmentShader = compileShader(GLES20.GL_FRAGMENT_SHADER, fragmentSrc)
            val program = GLES20.glCreateProgram()
            GLES20.glAttachShader(program, vertexShader)
            GLES20.glAttachShader(program, fragmentShader)
            GLES20.glLinkProgram(program)
            val status = IntArray(1)
            GLES20.glGetProgramiv(program, GLES20.GL_LINK_STATUS, status, 0)
            if (status[0] == 0) {
                val log = GLES20.glGetProgramInfoLog(program)
                GLES20.glDeleteProgram(program)
                throw RuntimeException("Program link failed: $log")
            }
            GLES20.glDeleteShader(vertexShader)
            GLES20.glDeleteShader(fragmentShader)
            return program
        }
    }
}
