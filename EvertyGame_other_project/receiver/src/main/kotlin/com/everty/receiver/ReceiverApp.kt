package com.everty.receiver

import com.everty.receiver.decoder.DecoderPreference
import com.everty.receiver.ui.DisplayScaleMode
import com.everty.receiver.ui.LowLatencyGameView
import com.everty.receiver.ui.VideoPanel
import java.awt.BorderLayout
import java.awt.Color
import java.awt.Dimension
import java.awt.FlowLayout
import java.awt.Font
import java.awt.GraphicsEnvironment
import java.awt.Rectangle
import java.awt.GridLayout
import java.awt.event.ActionEvent
import java.awt.event.InputEvent
import java.awt.event.KeyEvent
import java.awt.event.MouseAdapter
import java.awt.event.MouseEvent
import java.awt.event.WindowAdapter
import java.awt.event.WindowEvent
import java.net.NetworkInterface
import java.util.prefs.Preferences
import javax.swing.AbstractAction
import javax.swing.BorderFactory
import javax.swing.BoxLayout
import javax.swing.JButton
import javax.swing.JCheckBox
import javax.swing.JComboBox
import javax.swing.JComponent
import javax.swing.JFrame
import javax.swing.JLabel
import javax.swing.JOptionPane
import javax.swing.JPanel
import javax.swing.JTextField
import javax.swing.KeyStroke
import javax.swing.SwingUtilities
import javax.swing.UIManager

fun main() {
    SwingUtilities.invokeLater {
        UIManager.setLookAndFeel(UIManager.getSystemLookAndFeelClassName())
        ReceiverWindow().showWindow()
    }
}

private class ReceiverWindow {
    private val preferences = Preferences.userNodeForPackage(ReceiverWindow::class.java).node("receiver-window")
    private val videoPanel = VideoPanel()
    private val lowLatencyGameView = LowLatencyGameView()
    private val portField = JTextField(preferences.get("udpPort", "5001"), 8)
    private val scaleModeBox = JComboBox(DisplayScaleMode.entries.toTypedArray())
    private val decoderPreferenceBox = JComboBox(DecoderPreference.entries.toTypedArray())
    private val turboRealtimeCheck = JCheckBox("Turbo realtime", preferences.getBoolean("turboRealtime", true))
    private val autoFitWindowCheck = JCheckBox("Auto fit window", preferences.getBoolean("autoFitWindow", true))
    private val overlayHudCheck = JCheckBox("Overlay HUD", preferences.getBoolean("overlayHud", true))
    private val alwaysOnTopCheck = JCheckBox("Always on top", preferences.getBoolean("alwaysOnTop", false))
    private val matchStreamButton = JButton("Match stream")
    private val gameViewButton = JButton("Game view")
    private val fullscreenButton = JButton("Fullscreen")
    private val startButton = JButton("Start")
    private val stopButton = JButton("Stop")
    private val statusValue = JLabel("Idle")
    private val codecValue = JLabel("-")
    private val decodePathValue = JLabel("-")
    private val audioStatusValue = JLabel("-")
    private val audioQueueValue = JLabel("0")
    private val audioDropsValue = JLabel("0")
    private val presetValue = JLabel("-")
    private val resolutionValue = JLabel("-")
    private val targetFpsValue = JLabel("-")
    private val bitrateValue = JLabel("-")
    private val packetsValue = JLabel("0")
    private val assembledValue = JLabel("0")
    private val droppedValue = JLabel("0")
    private val decodedValue = JLabel("0")
    private val decodeFpsValue = JLabel("0")
    private val backlogValue = JLabel("0")
    private val queueDropsValue = JLabel("0")
    private val infoValue = JLabel(buildNetworkHint())
    private var currentStreamSize: Dimension? = null
    private var lastAutoFitSize: Dimension? = null
    private var fullscreen = false
    private var restoreBounds: Rectangle? = null
    private var restoreExtendedState: Int = JFrame.NORMAL
    private var suppressWindowClosed = false
    @Volatile
    private var autoOpenGameViewPending = false
    @Volatile
    private var lastConsolePreviewAtNs = 0L

    private val controller = ReceiverController(
        onSnapshot = ::renderSnapshot,
        onFrame = { frame ->
            if (autoOpenGameViewPending && !lowLatencyGameView.isVisible()) {
                autoOpenGameViewPending = false
                SwingUtilities.invokeLater {
                    if (!lowLatencyGameView.isVisible()) {
                        lowLatencyGameView.openOrFocus()
                        updateGameViewButton()
                    }
                }
            }
            val gameViewVisible = lowLatencyGameView.isVisible()
            if (gameViewVisible) {
                lowLatencyGameView.present(frame)
            }
            if (!gameViewVisible && shouldRefreshConsolePreview()) {
                videoPanel.updateFrame(frame)
            }
        },
        tryPresentDirect = { frame ->
            if (!turboRealtimeCheck.isSelected || !lowLatencyGameView.isVisible()) {
                false
            } else {
                runCatching {
                    lowLatencyGameView.present(frame)
                    true
                }.getOrDefault(false)
            }
        },
    )

    private val headerPanel = buildHeader()
    private val statsPanel = buildStatsPanel()
    private val frame = JFrame("Everty Receiver").apply {
        defaultCloseOperation = JFrame.DISPOSE_ON_CLOSE
        minimumSize = Dimension(820, 720)
        layout = BorderLayout(12, 12)
        addWindowListener(object : WindowAdapter() {
            override fun windowClosed(e: WindowEvent) {
                if (!suppressWindowClosed) {
                    controller.stop()
                    lowLatencyGameView.close()
                    videoPanel.close()
                }
            }
        })
    }

    fun showWindow() {
        frame.contentPane.add(headerPanel, BorderLayout.NORTH)
        frame.contentPane.add(videoPanel, BorderLayout.CENTER)
        frame.contentPane.add(statsPanel, BorderLayout.SOUTH)
        frame.isAlwaysOnTop = alwaysOnTopCheck.isSelected
        frame.pack()
        frame.setLocationRelativeTo(null)
        frame.isVisible = true

        val initialScaleMode = DisplayScaleMode.entries.firstOrNull {
            it.name == preferences.get("scaleMode", DisplayScaleMode.FIT.name)
        } ?: DisplayScaleMode.FIT
        val initialDecoderPreference = DecoderPreference.entries.firstOrNull {
            it.name == preferences.get("decoderPreference", DecoderPreference.AUTO.name)
        } ?: DecoderPreference.AUTO
        scaleModeBox.selectedItem = initialScaleMode
        decoderPreferenceBox.selectedItem = initialDecoderPreference
        videoPanel.setDisplayScaleMode(initialScaleMode)
        applyOverlayHudState()
        updateGameViewButton()
        bindHotkeys()

        videoPanel.addMouseListener(object : MouseAdapter() {
            override fun mouseClicked(e: MouseEvent) {
                if (e.button == MouseEvent.BUTTON1 && e.clickCount == 2) {
                    toggleFullscreen()
                }
            }
        })

        scaleModeBox.addActionListener {
            val selected = scaleModeBox.selectedItem as? DisplayScaleMode ?: DisplayScaleMode.FIT
            videoPanel.setDisplayScaleMode(selected)
            preferences.put("scaleMode", selected.name)
        }
        decoderPreferenceBox.addActionListener {
            val selected = decoderPreferenceBox.selectedItem as? DecoderPreference ?: DecoderPreference.AUTO
            preferences.put("decoderPreference", selected.name)
            controller.updateDecoderPreference(selected)
        }
        turboRealtimeCheck.addActionListener {
            preferences.putBoolean("turboRealtime", turboRealtimeCheck.isSelected)
            controller.updateUltraRealtime(turboRealtimeCheck.isSelected)
        }
        autoFitWindowCheck.addActionListener {
            preferences.putBoolean("autoFitWindow", autoFitWindowCheck.isSelected)
            if (autoFitWindowCheck.isSelected) {
                fitWindowToCurrentStream(force = true)
            }
        }
        overlayHudCheck.addActionListener {
            applyOverlayHudState()
            preferences.putBoolean("overlayHud", overlayHudCheck.isSelected)
        }
        alwaysOnTopCheck.addActionListener {
            frame.isAlwaysOnTop = alwaysOnTopCheck.isSelected || fullscreen
            preferences.putBoolean("alwaysOnTop", alwaysOnTopCheck.isSelected)
        }
        matchStreamButton.isEnabled = false
        matchStreamButton.addActionListener {
            fitWindowToCurrentStream(force = true)
        }
        gameViewButton.addActionListener {
            lowLatencyGameView.toggleVisibility()
            updateGameViewButton()
        }
        fullscreenButton.addActionListener {
            toggleFullscreen()
        }

        stopButton.isEnabled = false
        startButton.addActionListener {
            val port = portField.text.trim().toIntOrNull()
            if (port == null || port !in 1..65535) {
                JOptionPane.showMessageDialog(frame, "Enter a valid UDP port", "Invalid port", JOptionPane.ERROR_MESSAGE)
                return@addActionListener
            }
            preferences.put("udpPort", port.toString())
            autoOpenGameViewPending = true
            startButton.isEnabled = false
            stopButton.isEnabled = true
            statusValue.text = "Starting UDP listener on $port..."
            try {
                val decoderPreference = decoderPreferenceBox.selectedItem as? DecoderPreference ?: DecoderPreference.AUTO
                controller.start(port, decoderPreference, turboRealtimeCheck.isSelected)
            } catch (t: Throwable) {
                controller.stop()
                startButton.isEnabled = true
                stopButton.isEnabled = false
                statusValue.text = t.message ?: "Failed to start receiver"
                JOptionPane.showMessageDialog(
                    frame,
                    t.message ?: "Failed to start receiver",
                    "Receiver start error",
                    JOptionPane.ERROR_MESSAGE,
                )
            }
        }
        stopButton.addActionListener {
            autoOpenGameViewPending = false
            controller.stop()
            startButton.isEnabled = true
            stopButton.isEnabled = false
        }
    }

    private fun buildHeader(): JPanel {
        val root = JPanel(BorderLayout(12, 12)).apply {
            border = BorderFactory.createEmptyBorder(14, 14, 0, 14)
        }

        val titlePanel = JPanel().apply {
            layout = BoxLayout(this, BoxLayout.Y_AXIS)
        }
        titlePanel.add(JLabel("Everty Receiver").apply {
            font = font.deriveFont(Font.BOLD, 22f)
        })
        titlePanel.add(JLabel("UDP ingest, frame reassembly, hardware-decoder fallback, low-latency preview"))
        titlePanel.add(JLabel("Ctrl+G game view, Ctrl+Shift+G game fullscreen, F11 console fullscreen"))
        titlePanel.add(JLabel("When Game view is open, console preview is throttled to prioritize lower latency"))
        titlePanel.add(infoValue.apply {
            foreground = Color(0x2E5C2B)
        })

        val controls = JPanel(FlowLayout(FlowLayout.RIGHT)).apply {
            add(JLabel("View"))
            add(scaleModeBox)
            add(JLabel("Decoder"))
            add(decoderPreferenceBox)
            add(turboRealtimeCheck)
            add(autoFitWindowCheck)
            add(overlayHudCheck)
            add(alwaysOnTopCheck)
            add(matchStreamButton)
            add(gameViewButton)
            add(fullscreenButton)
            add(JLabel("UDP port"))
            add(portField)
            add(startButton)
            add(stopButton)
        }

        root.add(titlePanel, BorderLayout.WEST)
        root.add(controls, BorderLayout.EAST)
        return root
    }

    private fun buildStatsPanel(): JPanel {
        val panel = JPanel(GridLayout(0, 4, 12, 8)).apply {
            border = BorderFactory.createEmptyBorder(0, 14, 14, 14)
        }

        addStat(panel, "Status", statusValue)
        addStat(panel, "Codec", codecValue)
        addStat(panel, "Decode Path", decodePathValue)
        addStat(panel, "Audio", audioStatusValue)
        addStat(panel, "Preset", presetValue)
        addStat(panel, "Resolution", resolutionValue)
        addStat(panel, "Target FPS", targetFpsValue)
        addStat(panel, "Bitrate", bitrateValue)
        addStat(panel, "Audio queue", audioQueueValue)
        addStat(panel, "Audio drops", audioDropsValue)
        addStat(panel, "UDP packets", packetsValue)
        addStat(panel, "Frames assembled", assembledValue)
        addStat(panel, "Frames dropped", droppedValue)
        addStat(panel, "Frames decoded", decodedValue)
        addStat(panel, "Decode FPS", decodeFpsValue)
        addStat(panel, "Decode backlog", backlogValue)
        addStat(panel, "Queue drops", queueDropsValue)

        return panel
    }

    private fun addStat(root: JPanel, label: String, valueLabel: JLabel) {
        val card = JPanel().apply {
            layout = BoxLayout(this, BoxLayout.Y_AXIS)
            border = BorderFactory.createCompoundBorder(
                BorderFactory.createLineBorder(java.awt.Color(0xD0D7DE)),
                BorderFactory.createEmptyBorder(10, 12, 10, 12),
            )
        }
        card.add(JLabel(label))
        card.add(valueLabel.apply {
            font = font.deriveFont(Font.BOLD, 16f)
        })
        root.add(card)
    }

    private fun renderSnapshot(snapshot: ReceiverSnapshot) {
        statusValue.text = snapshot.status
        codecValue.text = snapshot.sessionCodec
        decodePathValue.text = snapshot.decodePath
        audioStatusValue.text = snapshot.audioStatus
        presetValue.text = snapshot.sessionPreset
        resolutionValue.text = snapshot.resolution
        targetFpsValue.text = if (snapshot.fpsTarget == 0) "-" else snapshot.fpsTarget.toString()
        bitrateValue.text = if (snapshot.bitrateMbps == 0.0) "-" else String.format("%.1f Mbps", snapshot.bitrateMbps)
        packetsValue.text = snapshot.packetsReceived.toString()
        assembledValue.text = snapshot.framesAssembled.toString()
        droppedValue.text = snapshot.framesDropped.toString()
        decodedValue.text = snapshot.framesDecoded.toString()
        decodeFpsValue.text = snapshot.decodeFps.toString()
        backlogValue.text = if (snapshot.decoderWaitingForKeyFrame) {
            "resync (${snapshot.decoderBacklogFrames}f/${snapshot.decoderBacklogKb} KB)"
        } else {
            "${snapshot.decoderBacklogFrames}f / ${snapshot.decoderBacklogKb} KB"
        }
        queueDropsValue.text = snapshot.decoderQueueDrops.toString()
        audioQueueValue.text = "${snapshot.audioQueuedMs} ms"
        audioDropsValue.text = snapshot.audioDroppedChunks.toString()
        startButton.isEnabled = !snapshot.listening
        stopButton.isEnabled = snapshot.listening
        videoPanel.setOverlayLines(buildOverlayLines(snapshot))
        lowLatencyGameView.updateSnapshot(snapshot)
        updateGameViewButton()

        val streamSize = parseResolution(snapshot.resolution)
        currentStreamSize = streamSize
        matchStreamButton.isEnabled = streamSize != null && !fullscreen
        lowLatencyGameView.updateStreamSize(streamSize)
        if (!snapshot.listening && streamSize == null) {
            lastAutoFitSize = null
        }
        if (autoFitWindowCheck.isSelected && !fullscreen && streamSize != null && shouldAutoFit(streamSize)) {
            fitWindowToStream(streamSize)
        }
    }

    private fun shouldAutoFit(streamSize: Dimension): Boolean {
        val previous = lastAutoFitSize ?: return true
        return previous.width != streamSize.width || previous.height != streamSize.height
    }

    private fun fitWindowToCurrentStream(force: Boolean = false) {
        val streamSize = currentStreamSize ?: return
        if (!force && !autoFitWindowCheck.isSelected) {
            return
        }
        fitWindowToStream(streamSize)
    }

    private fun fitWindowToStream(streamSize: Dimension) {
        if (fullscreen) {
            return
        }

        val screenBounds = GraphicsEnvironment.getLocalGraphicsEnvironment().maximumWindowBounds
        val maxVideoWidth = (screenBounds.width * 0.82).toInt().coerceAtLeast(480)
        val reservedHeight = headerPanel.preferredSize.height + statsPanel.preferredSize.height + 96
        val maxVideoHeight = (screenBounds.height - reservedHeight).coerceAtLeast(320)

        val scale = minOf(
            maxVideoWidth.toDouble() / streamSize.width,
            maxVideoHeight.toDouble() / streamSize.height,
        )

        val targetVideoWidth = (streamSize.width * scale).toInt().coerceAtLeast(360)
        val targetVideoHeight = (streamSize.height * scale).toInt().coerceAtLeast(240)
        videoPanel.preferredSize = Dimension(targetVideoWidth, targetVideoHeight)
        frame.pack()
        frame.setLocationRelativeTo(null)
        lastAutoFitSize = Dimension(streamSize)
    }

    private fun parseResolution(raw: String): Dimension? {
        val parts = raw.split('x')
        if (parts.size != 2) {
            return null
        }

        val width = parts[0].trim().toIntOrNull() ?: return null
        val height = parts[1].trim().toIntOrNull() ?: return null
        if (width <= 0 || height <= 0) {
            return null
        }
        return Dimension(width, height)
    }

    private fun applyOverlayHudState() {
        videoPanel.setOverlayVisible(overlayHudCheck.isSelected)
    }

    private fun shouldRefreshConsolePreview(): Boolean {
        val now = System.nanoTime()
        if (now - lastConsolePreviewAtNs >= 100_000_000L) {
            lastConsolePreviewAtNs = now
            return true
        }
        return false
    }

    private fun updateGameViewButton() {
        val backend = lowLatencyGameView.backendLabel()
        val suffix = if (backend == "Closed") "" else " [$backend]"
        gameViewButton.text = if (lowLatencyGameView.isVisible()) "Hide view$suffix" else "Game view$suffix"
    }

    private fun bindHotkeys() {
        val inputMap = frame.rootPane.getInputMap(JComponent.WHEN_IN_FOCUSED_WINDOW)
        val actionMap = frame.rootPane.actionMap

        fun register(actionKey: String, keyStroke: KeyStroke, action: () -> Unit) {
            inputMap.put(keyStroke, actionKey)
            actionMap.put(actionKey, object : AbstractAction() {
                override fun actionPerformed(e: ActionEvent?) {
                    action()
                }
            })
        }

        register("toggle-console-fullscreen-f11", KeyStroke.getKeyStroke(KeyEvent.VK_F11, 0)) {
            toggleFullscreen()
        }
        register("toggle-console-fullscreen-alt-enter", KeyStroke.getKeyStroke(KeyEvent.VK_ENTER, InputEvent.ALT_DOWN_MASK)) {
            toggleFullscreen()
        }
        register("exit-console-fullscreen", KeyStroke.getKeyStroke(KeyEvent.VK_ESCAPE, 0)) {
            if (fullscreen) {
                toggleFullscreen()
            }
        }
        register("toggle-overlay", KeyStroke.getKeyStroke(KeyEvent.VK_H, InputEvent.CTRL_DOWN_MASK)) {
            overlayHudCheck.isSelected = !overlayHudCheck.isSelected
            applyOverlayHudState()
            preferences.putBoolean("overlayHud", overlayHudCheck.isSelected)
        }
        register("scale-fit", KeyStroke.getKeyStroke(KeyEvent.VK_1, InputEvent.CTRL_DOWN_MASK)) {
            scaleModeBox.selectedItem = DisplayScaleMode.FIT
        }
        register("scale-fill", KeyStroke.getKeyStroke(KeyEvent.VK_2, InputEvent.CTRL_DOWN_MASK)) {
            scaleModeBox.selectedItem = DisplayScaleMode.FILL
        }
        register("scale-stretch", KeyStroke.getKeyStroke(KeyEvent.VK_3, InputEvent.CTRL_DOWN_MASK)) {
            scaleModeBox.selectedItem = DisplayScaleMode.STRETCH
        }
        register("toggle-game-view", KeyStroke.getKeyStroke(KeyEvent.VK_G, InputEvent.CTRL_DOWN_MASK)) {
            lowLatencyGameView.toggleVisibility()
            updateGameViewButton()
        }
        register("toggle-game-fullscreen", KeyStroke.getKeyStroke(KeyEvent.VK_G, InputEvent.CTRL_DOWN_MASK or InputEvent.SHIFT_DOWN_MASK)) {
            lowLatencyGameView.toggleFullscreen()
            updateGameViewButton()
        }
    }

    private fun toggleFullscreen() {
        if (fullscreen) {
            exitFullscreen()
        } else {
            enterFullscreen()
        }
    }

    private fun enterFullscreen() {
        if (fullscreen) {
            return
        }

        restoreBounds = frame.bounds
        restoreExtendedState = frame.extendedState
        fullscreen = true
        fullscreenButton.text = "Windowed"
        headerPanel.isVisible = false
        statsPanel.isVisible = false
        reconfigureFrame {
            frame.isUndecorated = true
            frame.extendedState = JFrame.NORMAL
            frame.bounds = frame.graphicsConfiguration.bounds
            frame.isAlwaysOnTop = true
        }
    }

    private fun exitFullscreen() {
        if (!fullscreen) {
            return
        }

        fullscreen = false
        fullscreenButton.text = "Fullscreen"
        headerPanel.isVisible = true
        statsPanel.isVisible = true
        reconfigureFrame {
            frame.isUndecorated = false
            val previousBounds = restoreBounds
            if (previousBounds != null) {
                frame.bounds = previousBounds
            } else {
                frame.pack()
                frame.setLocationRelativeTo(null)
            }
            frame.extendedState = restoreExtendedState
            frame.isAlwaysOnTop = alwaysOnTopCheck.isSelected
        }
    }

    private fun reconfigureFrame(updateWindow: () -> Unit) {
        suppressWindowClosed = true
        frame.dispose()
        updateWindow()
        frame.isVisible = true
        frame.contentPane.revalidate()
        frame.repaint()
        suppressWindowClosed = false
    }

    private fun buildOverlayLines(snapshot: ReceiverSnapshot): List<String> {
        if (!snapshot.listening) {
            return listOf(snapshot.status)
        }

        val streamLine = buildString {
            append(snapshot.sessionCodec)
            if (snapshot.resolution != "-") {
                append("  ")
                append(snapshot.resolution)
            }
            if (snapshot.fpsTarget > 0) {
                append("  ")
                append(snapshot.fpsTarget)
                append(" fps")
            }
            if (snapshot.bitrateMbps > 0.0) {
                append("  ")
                append(String.format("%.1f Mbps", snapshot.bitrateMbps))
            }
        }

        val decodeLine = buildString {
            append(snapshot.decodePath)
            append("  decode ")
            append(snapshot.decodeFps)
            append(" fps")
            append("  backlog ")
            append(snapshot.decoderBacklogFrames)
            append("f/")
            append(snapshot.decoderBacklogKb)
            append(" KB")
            append("  drops ")
            append(snapshot.decoderQueueDrops)
        }

        val audioLine = buildString {
            append(snapshot.audioStatus)
            append("  queue ")
            append(snapshot.audioQueuedMs)
            append(" ms")
            append("  drops ")
            append(snapshot.audioDroppedChunks)
        }

        return listOf(snapshot.status, streamLine, decodeLine, audioLine)
    }

    private fun buildNetworkHint(): String {
        val addresses = NetworkInterface.getNetworkInterfaces()
            .toList()
            .flatMap { networkInterface ->
                networkInterface.inetAddresses.toList()
                    .filter { address ->
                        !address.isLoopbackAddress &&
                            address.hostAddress?.contains(':') == false
                    }
                    .map { address -> address.hostAddress }
            }
            .distinct()

        return if (addresses.isEmpty()) {
            "Receiver only listens after Start. Then point Android sender to this PC IP."
        } else {
            "Start only opens UDP listener. Use one of these PC IPs in sender: ${addresses.joinToString()}"
        }
    }
}
