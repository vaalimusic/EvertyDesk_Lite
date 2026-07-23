package com.everty.evertygame.stream

import android.graphics.SurfaceTexture
import android.opengl.EGL14
import android.opengl.EGLConfig
import android.opengl.EGLContext
import android.opengl.EGLDisplay
import android.opengl.EGLExt
import android.opengl.EGLSurface
import android.opengl.GLES11Ext
import android.opengl.GLES20
import android.os.Handler
import android.view.Surface
import com.everty.evertygame.touch.TouchLatencySprintController
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer

internal class SplitStreamGlHub(
    private val callbackHandler: Handler,
    private val captureWidth: Int,
    private val captureHeight: Int,
    private val baseSurfaceWidth: Int,
    private val baseSurfaceHeight: Int,
    private val enhancementSurfaceWidth: Int,
    private val enhancementSurfaceHeight: Int,
    private val screenWidth: Int,
    private val screenHeight: Int,
    private val baseInputSurface: Surface,
    private val enhancementInputSurface: Surface,
    private val roiProvider: () -> TouchLatencySprintController.RoiSnapshot?,
    private val onEnhancementRendered: (Long, TouchLatencySprintController.RoiSnapshot) -> Unit,
) : SurfaceTexture.OnFrameAvailableListener, AutoCloseable {
    private val texMatrix = FloatArray(16)
    private val fullscreenVertices: FloatBuffer = ByteBuffer
        .allocateDirect(4 * 4 * 4)
        .order(ByteOrder.nativeOrder())
        .asFloatBuffer()
        .apply {
            put(
                floatArrayOf(
                    -1f, -1f, 0f, 0f,
                    1f, -1f, 1f, 0f,
                    -1f, 1f, 0f, 1f,
                    1f, 1f, 1f, 1f,
                ),
            )
            position(0)
        }

    private var display: EGLDisplay = EGL14.EGL_NO_DISPLAY
    private var context: EGLContext = EGL14.EGL_NO_CONTEXT
    private var baseEglSurface: EGLSurface = EGL14.EGL_NO_SURFACE
    private var enhancementEglSurface: EGLSurface = EGL14.EGL_NO_SURFACE
    private var externalTextureId = 0
    private var program = 0
    private var positionHandle = 0
    private var texCoordHandle = 0
    private var texMatrixHandle = 0
    private var cropRectHandle = 0
    private var samplerHandle = 0
    private var surfaceTexture: SurfaceTexture? = null
    private var captureSurface: Surface? = null
    private var released = false
    private var framePending = false

    init {
        initializeGl()
    }

    val inputSurface: Surface
        get() = captureSurface ?: error("SplitStreamGlHub surface is not available")

    override fun onFrameAvailable(surfaceTexture: SurfaceTexture?) {
        framePending = true
        drainFrame()
    }

    fun drainFrame() {
        if (released || !framePending) {
            return
        }

        framePending = false
        val texture = surfaceTexture ?: return
        texture.updateTexImage()
        texture.getTransformMatrix(texMatrix)
        val timestampNs = texture.timestamp

        renderBase(timestampNs)
        val roiSnapshot = roiProvider()
        if (roiSnapshot?.isActive == true) {
            renderEnhancement(timestampNs, roiSnapshot)
            onEnhancementRendered(timestampNs / 1_000L, roiSnapshot)
        }
    }

    override fun close() {
        if (released) {
            return
        }

        released = true
        surfaceTexture?.setOnFrameAvailableListener(null)
        captureSurface?.release()
        surfaceTexture?.release()
        captureSurface = null
        surfaceTexture = null

        if (display !== EGL14.EGL_NO_DISPLAY) {
            EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT)
            if (baseEglSurface !== EGL14.EGL_NO_SURFACE) {
                EGL14.eglDestroySurface(display, baseEglSurface)
            }
            if (enhancementEglSurface !== EGL14.EGL_NO_SURFACE) {
                EGL14.eglDestroySurface(display, enhancementEglSurface)
            }
            if (context !== EGL14.EGL_NO_CONTEXT) {
                EGL14.eglDestroyContext(display, context)
            }
            EGL14.eglTerminate(display)
        }

        if (externalTextureId != 0) {
            GLES20.glDeleteTextures(1, intArrayOf(externalTextureId), 0)
        }
        if (program != 0) {
            GLES20.glDeleteProgram(program)
        }

        display = EGL14.EGL_NO_DISPLAY
        context = EGL14.EGL_NO_CONTEXT
        baseEglSurface = EGL14.EGL_NO_SURFACE
        enhancementEglSurface = EGL14.EGL_NO_SURFACE
        externalTextureId = 0
        program = 0
    }

    private fun initializeGl() {
        display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY)
        check(display != EGL14.EGL_NO_DISPLAY) { "Failed to get EGL display" }
        val version = IntArray(2)
        check(EGL14.eglInitialize(display, version, 0, version, 1)) { "Failed to initialize EGL" }

        val config = chooseConfig()
        context = EGL14.eglCreateContext(
            display,
            config,
            EGL14.EGL_NO_CONTEXT,
            intArrayOf(EGL14.EGL_CONTEXT_CLIENT_VERSION, 2, EGL14.EGL_NONE),
            0,
        )
        check(context != EGL14.EGL_NO_CONTEXT) { "Failed to create EGL context" }

        baseEglSurface = createWindowSurface(config, baseInputSurface)
        enhancementEglSurface = createWindowSurface(config, enhancementInputSurface)

        externalTextureId = createExternalTexture()
        surfaceTexture = SurfaceTexture(externalTextureId).apply {
            setDefaultBufferSize(captureWidth, captureHeight)
            setOnFrameAvailableListener(this@SplitStreamGlHub, callbackHandler)
        }
        captureSurface = Surface(surfaceTexture)

        makeCurrent(baseEglSurface)
        program = createProgram(VERTEX_SHADER, FRAGMENT_SHADER)
        positionHandle = GLES20.glGetAttribLocation(program, "aPosition")
        texCoordHandle = GLES20.glGetAttribLocation(program, "aTexCoord")
        texMatrixHandle = GLES20.glGetUniformLocation(program, "uTexMatrix")
        cropRectHandle = GLES20.glGetUniformLocation(program, "uCropRect")
        samplerHandle = GLES20.glGetUniformLocation(program, "uTexture")
    }

    private fun renderBase(timestampNs: Long) {
        makeCurrent(baseEglSurface)
        GLES20.glViewport(0, 0, baseSurfaceWidth, baseSurfaceHeight)
        drawToCurrentSurface(0f, 0f, 1f, 1f)
        EGLExt.eglPresentationTimeANDROID(display, baseEglSurface, timestampNs)
        EGL14.eglSwapBuffers(display, baseEglSurface)
    }

    private fun renderEnhancement(timestampNs: Long, roiSnapshot: TouchLatencySprintController.RoiSnapshot) {
        makeCurrent(enhancementEglSurface)
        val left = roiSnapshot.rect.left / screenWidth.toFloat()
        val top = roiSnapshot.rect.top / screenHeight.toFloat()
        val right = roiSnapshot.rect.right / screenWidth.toFloat()
        val bottom = roiSnapshot.rect.bottom / screenHeight.toFloat()
        GLES20.glViewport(0, 0, enhancementSurfaceWidth, enhancementSurfaceHeight)
        drawToCurrentSurface(left, top, right, bottom)
        EGLExt.eglPresentationTimeANDROID(display, enhancementEglSurface, timestampNs)
        EGL14.eglSwapBuffers(display, enhancementEglSurface)
    }

    private fun drawToCurrentSurface(cropLeft: Float, cropTop: Float, cropRight: Float, cropBottom: Float) {
        GLES20.glUseProgram(program)
        GLES20.glDisable(GLES20.GL_BLEND)
        GLES20.glClearColor(0f, 0f, 0f, 1f)
        GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT)

        fullscreenVertices.position(0)
        GLES20.glVertexAttribPointer(positionHandle, 2, GLES20.GL_FLOAT, false, 16, fullscreenVertices)
        GLES20.glEnableVertexAttribArray(positionHandle)
        fullscreenVertices.position(2)
        GLES20.glVertexAttribPointer(texCoordHandle, 2, GLES20.GL_FLOAT, false, 16, fullscreenVertices)
        GLES20.glEnableVertexAttribArray(texCoordHandle)

        GLES20.glUniformMatrix4fv(texMatrixHandle, 1, false, texMatrix, 0)
        GLES20.glUniform4f(cropRectHandle, cropLeft, cropTop, cropRight, cropBottom)
        GLES20.glActiveTexture(GLES20.GL_TEXTURE0)
        GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, externalTextureId)
        GLES20.glUniform1i(samplerHandle, 0)
        GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4)
        GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, 0)
        GLES20.glDisableVertexAttribArray(positionHandle)
        GLES20.glDisableVertexAttribArray(texCoordHandle)
    }

    private fun chooseConfig(): EGLConfig {
        val configs = arrayOfNulls<EGLConfig>(1)
        val count = IntArray(1)
        val attributes = intArrayOf(
            EGL14.EGL_RED_SIZE, 8,
            EGL14.EGL_GREEN_SIZE, 8,
            EGL14.EGL_BLUE_SIZE, 8,
            EGL14.EGL_ALPHA_SIZE, 8,
            EGL14.EGL_RENDERABLE_TYPE, EGL14.EGL_OPENGL_ES2_BIT,
            EGL14.EGL_SURFACE_TYPE, EGL14.EGL_WINDOW_BIT,
            EGL_RECORDABLE_ANDROID, 1,
            EGL14.EGL_NONE,
        )
        check(EGL14.eglChooseConfig(display, attributes, 0, configs, 0, configs.size, count, 0) && count[0] > 0) {
            "Failed to choose EGL config"
        }
        return configs[0] ?: error("EGL config is null")
    }

    private fun createWindowSurface(config: EGLConfig, surface: Surface): EGLSurface {
        val eglSurface = EGL14.eglCreateWindowSurface(display, config, surface, intArrayOf(EGL14.EGL_NONE), 0)
        check(eglSurface != EGL14.EGL_NO_SURFACE) { "Failed to create EGL window surface" }
        return eglSurface
    }

    private fun makeCurrent(surface: EGLSurface) {
        check(EGL14.eglMakeCurrent(display, surface, surface, context)) { "Failed to make EGL context current" }
    }

    private fun createExternalTexture(): Int {
        val textures = IntArray(1)
        GLES20.glGenTextures(1, textures, 0)
        GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, textures[0])
        GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR)
        GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR)
        GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE)
        GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE)
        GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, 0)
        return textures[0]
    }

    private fun createProgram(vertexShaderSource: String, fragmentShaderSource: String): Int {
        val vertexShader = compileShader(GLES20.GL_VERTEX_SHADER, vertexShaderSource)
        val fragmentShader = compileShader(GLES20.GL_FRAGMENT_SHADER, fragmentShaderSource)
        val createdProgram = GLES20.glCreateProgram()
        GLES20.glAttachShader(createdProgram, vertexShader)
        GLES20.glAttachShader(createdProgram, fragmentShader)
        GLES20.glLinkProgram(createdProgram)
        val linkStatus = IntArray(1)
        GLES20.glGetProgramiv(createdProgram, GLES20.GL_LINK_STATUS, linkStatus, 0)
        GLES20.glDeleteShader(vertexShader)
        GLES20.glDeleteShader(fragmentShader)
        check(linkStatus[0] == GLES20.GL_TRUE) {
            "Failed to link GL program: ${GLES20.glGetProgramInfoLog(createdProgram)}"
        }
        return createdProgram
    }

    private fun compileShader(type: Int, source: String): Int {
        val shader = GLES20.glCreateShader(type)
        GLES20.glShaderSource(shader, source)
        GLES20.glCompileShader(shader)
        val compiled = IntArray(1)
        GLES20.glGetShaderiv(shader, GLES20.GL_COMPILE_STATUS, compiled, 0)
        check(compiled[0] == GLES20.GL_TRUE) {
            "Failed to compile shader: ${GLES20.glGetShaderInfoLog(shader)}"
        }
        return shader
    }

    private companion object {
        private const val EGL_RECORDABLE_ANDROID = 0x3142

        private const val VERTEX_SHADER = """
            attribute vec4 aPosition;
            attribute vec2 aTexCoord;
            uniform mat4 uTexMatrix;
            uniform vec4 uCropRect;
            varying vec2 vTexCoord;
            void main() {
                vec2 cropCoord = vec2(
                    mix(uCropRect.x, uCropRect.z, aTexCoord.x),
                    mix(uCropRect.y, uCropRect.w, aTexCoord.y)
                );
                vec4 transformed = uTexMatrix * vec4(cropCoord, 0.0, 1.0);
                vTexCoord = transformed.xy;
                gl_Position = aPosition;
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
    }
}
