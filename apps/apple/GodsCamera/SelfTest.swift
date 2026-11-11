import AVFoundation
import AppKit
import GodsCameraKit

/// A check the app runs on itself when started with `--selftest <report path>`.
///
/// It drives the real pipeline with the test pattern: through the engine, into
/// the installed Camera Extension, and, when the app may use cameras, back out
/// of the virtual camera the way a meeting app would read it. It writes what it
/// measured as JSON and quits. No real camera is opened.
///
/// `--soak <seconds>` makes the live part last that long, for endurance runs.
enum SelfTest {
    static var requestedReportPath: String? { argument(after: "--selftest") }

    /// How long the live part runs, in seconds.
    static var liveSeconds: Double { argument(after: "--soak").flatMap(Double.init) ?? 3 }

    fileprivate static func argument(after flag: String) -> String? {
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: flag), arguments.count > index + 1 else { return nil }
        return arguments[index + 1]
    }

    /// The memory this process is charged for, in megabytes.
    static func footprintMB() -> Double {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let result = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        return result == KERN_SUCCESS ? (Double(info.phys_footprint) / 1_048_576 * 10).rounded() / 10 : 0
    }

    static func run(reportPath: String) {
        let runner = Runner(reportPath: reportPath)
        retained = runner
        runner.start()
    }

    private static var retained: Runner?

    private final class Runner: NSObject, AVCaptureVideoDataOutputSampleBufferDelegate {
        private let reportPath: String
        private var report: [String: Any] = ["startedAt": ISO8601DateFormatter().string(from: Date())]
        private let work = DispatchQueue(label: "dev.godscamera.selftest")
        private let extensionManager = ExtensionManager()
        private let link = VirtualCameraLink()
        private var engine: CameraEngine?

        private let lock = NSLock()
        /// When each test-pattern frame left the engine, by frame number.
        private var sentAt: [UInt32: UInt64] = [:]
        private var engineFrames = 0
        private var latestStatus = CameraEngine.Status()
        private var clip: Clip?
        private var readBack: [(number: UInt32?, latencyMs: Double?, width: Int)] = []
        /// True only while live test-pattern frames are flowing, when a frame number maps to one send time.
        private var timingLive = false

        init(reportPath: String) {
            self.reportPath = reportPath
        }

        func start() {
            report["bundledExtensionBuild"] = ExtensionManager.bundledBuild
            ensureExtension { [self] state in
                report["extensionState"] = "\(state)"
                var handover = SelfTest.argument(after: "--handover").flatMap(Double.init)
                if extensionManager.replaced {
                    handover = extensionManager.lastHandover.map { ($0 * 100).rounded() / 100 } ?? -1
                }
                if let handover {
                    report["replacedOlderBuild"] = true
                    report["handoverSeconds"] = handover
                }
                report["reopenedAfterInstall"] = Relauncher.wasRelaunched
                // An app that was running during the install never sees the new
                // camera, so the check continues in a fresh copy of the app.
                if extensionManager.activatedInThisRun, !Relauncher.wasRelaunched {
                    DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
                        Relauncher.relaunch(extraArguments: handover.map { ["--handover", "\($0)"] } ?? [])
                    }
                    return
                }
                work.async { self.runPipeline() }
            }
        }

        /// Install or update the extension if the installed build differs, then report its state.
        private func ensureExtension(then done: @escaping (ExtensionManager.State) -> Void) {
            var finished = false
            var requestedActivation = false
            let finish: (ExtensionManager.State) -> Void = { state in
                guard !finished else { return }
                finished = true
                done(state)
            }
            extensionManager.onChange = { [self] state in
                switch state {
                case .installed, .waitingForApproval, .needsRestart, .failed:
                    finish(state)
                case .notInstalled, .outdated:
                    if requestedActivation {
                        finish(state)
                    } else {
                        requestedActivation = true
                        report["activationRequested"] = true
                        extensionManager.activate()
                    }
                case .unknown: break
                }
            }
            extensionManager.refresh()
            DispatchQueue.main.asyncAfter(deadline: .now() + 45) { finish(.failed("timed out waiting for macOS")) }
        }

        private func wait(_ seconds: Double) { Thread.sleep(forTimeInterval: seconds) }

        private func status() -> CameraEngine.Status {
            lock.lock()
            defer { lock.unlock() }
            return latestStatus
        }

        private func runPipeline() {
            defer { finish() }
            // The camera can take a while to appear after an install or update.
            var listed = false
            let waitStarted = Date()
            for _ in 0..<200 where !listed {
                listed = VirtualCameraLink.isInstalled
                if !listed { wait(0.2) }
            }
            report["virtualCameraListed"] = listed
            report["listedAfterSeconds"] = (Date().timeIntervalSince(waitStarted) * 10).rounded() / 10
            var cameraEvents = 0
            // Its own queue: this one is busy waiting, and the events must still be counted.
            let watcher = VirtualCameraWatcher(queue: DispatchQueue(label: "dev.godscamera.selftest.watcher")) {}
            defer { report["cameraChangeEvents"] = max(cameraEvents, watcher.cameraEvents) }

            guard let renderer = try? MetalRenderer(), let pattern = TestPatternSource(size: .hd1080, frameRate: 30) else {
                report["error"] = "renderer or test pattern unavailable"
                return
            }
            let engine = CameraEngine(renderer: renderer)
            self.engine = engine
            engine.sink = link
            engine.onStatus = { [self] status in
                lock.lock()
                latestStatus = status
                lock.unlock()
            }
            engine.onClip = { [self] clip in
                lock.lock()
                self.clip = clip
                lock.unlock()
            }
            engine.onOutput = { [self] buffer, hostTimeNs in
                let number = TestPatternSource.readFrameNumber(from: buffer)
                lock.lock()
                engineFrames += 1
                // The number wraps after 65,536 frames, so the latest send time for a number wins.
                if let number, timingLive { sentAt[number] = hostTimeNs }
                lock.unlock()
            }

            if listed {
                do {
                    try link.open()
                    report["linkOpened"] = true
                } catch {
                    report["linkOpened"] = false
                    report["linkError"] = "\(error)"
                }
            }
            let statsBefore = VirtualCameraLink.readStats()
            report["statsReadable"] = statsBefore != nil
            if let stats = statsBefore {
                report["extensionBuild"] = stats.build
                report["extensionPid"] = Int(stats.pid)
                report["extensionFormat"] = "\(stats.width)x\(stats.height)@\(Int(stats.frameRate.rounded()))"
            }
            // The block list: set it, read it back, put the old one back.
            if let before = VirtualCameraLink.blockedPids() {
                let probe = before.union([1])
                let accepted = VirtualCameraLink.setBlockedPids(probe)
                report["blockListRoundTrip"] = accepted && VirtualCameraLink.blockedPids() == probe
                VirtualCameraLink.setBlockedPids(before)
            }

            // 1. Live for three seconds.
            engine.setSource(pattern)
            let access = AVCaptureDevice.authorizationStatus(for: .video)
            let authorized = access == .authorized
            report["cameraAccess"] =
                [.authorized: "authorized", .denied: "denied", .restricted: "restricted", .notDetermined: "not asked yet"][
                    access] ?? "unknown"
            let session = authorized && listed ? startReadBack() : nil
            report["readBackStarted"] = session != nil
            let memoryBefore = SelfTest.footprintMB()
            lock.lock()
            timingLive = true
            lock.unlock()
            wait(SelfTest.liveSeconds)
            lock.lock()
            timingLive = false
            let liveFrames = engineFrames
            lock.unlock()
            cameraEvents = watcher.cameraEvents
            report["liveSeconds"] = SelfTest.liveSeconds
            report["liveFrames"] = liveFrames
            report["liveFps"] = (Double(liveFrames) / SelfTest.liveSeconds * 10).rounded() / 10
            report["renderMs"] = (status().renderMs * 100).rounded() / 100
            report["memoryStartMB"] = memoryBefore
            report["memoryEndMB"] = SelfTest.footprintMB()
            if let during = VirtualCameraLink.readStats() {
                report["writerVerified"] = during.writerVerified
                report["extensionClientsWhileLive"] = during.clients
                report["extensionReaders"] = during.clientList.map { AppIdentity.of(pid: $0.pid)?.name ?? "pid \($0.pid)" }
                report["extensionConnectedApps"] = Array(Set(during.connected.compactMap { AppIdentity.of(pid: $0)?.name })).sorted()
            }

            // 2. Record two seconds, loop it for three, return to live.
            engine.startRecording()
            wait(2)
            engine.stopRecording()
            wait(0.5)
            lock.lock()
            let recorded = clip
            lock.unlock()
            if let recorded {
                report["clipFrames"] = recorded.frameCount
                report["loopStart"] = recorded.loop.start
                report["loopEnd"] = recorded.loop.end
                engine.startLoop(recorded)
                wait(3)
                report["modeWhileLooping"] = "\(status().mode)"
                engine.stopLoop()
                wait(1)
                report["modeAfterLoop"] = "\(status().mode)"
            } else {
                report["error"] = "no clip was recorded"
            }

            // 3. What the extension saw.
            wait(0.5)
            if let before = statsBefore, let after = VirtualCameraLink.readStats() {
                report["extensionFramesIn"] = after.framesIn - before.framesIn
                report["extensionFramesOut"] = after.framesOut - before.framesOut
                report["extensionClients"] = after.clients
                report["extensionLatencyMs"] = after.latencyMs
                report["extensionMaxLatencyMs"] = after.maxLatencyMs
            }
            report["linkFramesSent"] = link.framesSent
            report["linkFramesDropped"] = link.framesDropped

            // 4. What a client app would have received.
            session?.stopRunning()
            lock.lock()
            let received = readBack
            lock.unlock()
            if session != nil {
                let latencies = received.compactMap(\.latencyMs).sorted()
                report["readBackFrames"] = received.count
                report["readBackWidth"] = received.last?.width ?? 0
                report["readBackNumbered"] = received.compactMap(\.number).count
                if !latencies.isEmpty {
                    report["endToEndMedianMs"] = (latencies[latencies.count / 2] * 10).rounded() / 10
                    report["endToEndP95Ms"] = (latencies[min(latencies.count - 1, latencies.count * 95 / 100)] * 10).rounded() / 10
                }
            }
            engine.setSource(nil)
            engine.sync()
            link.close()
        }

        /// Read the virtual camera the way a meeting app does.
        private func startReadBack() -> AVCaptureSession? {
            guard let device = AVCaptureDevice(uniqueID: CameraConfig.deviceUID.uuidString),
                let input = try? AVCaptureDeviceInput(device: device)
            else { return nil }
            let session = AVCaptureSession()
            session.beginConfiguration()
            guard session.canAddInput(input) else { return nil }
            session.addInput(input)
            let output = AVCaptureVideoDataOutput()
            output.videoSettings = [kCVPixelBufferPixelFormatTypeKey as String: kCVPixelFormatType_32BGRA]
            output.setSampleBufferDelegate(self, queue: DispatchQueue(label: "dev.godscamera.selftest.readback"))
            guard session.canAddOutput(output) else { return nil }
            session.addOutput(output)
            session.commitConfiguration()
            session.startRunning()
            return session
        }

        func captureOutput(
            _ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection
        ) {
            guard let buffer = CMSampleBufferGetImageBuffer(sampleBuffer) else { return }
            let now = hostTimeNanoseconds()
            let number = TestPatternSource.readFrameNumber(from: buffer)
            lock.lock()
            var latency: Double?
            if timingLive, let number, let sent = sentAt[number], now >= sent, now - sent < 2_000_000_000 {
                latency = Double(now - sent) / 1_000_000
            }
            readBack.append((number, latency, CVPixelBufferGetWidth(buffer)))
            lock.unlock()
        }

        private func finish() {
            report["finishedAt"] = ISO8601DateFormatter().string(from: Date())
            if let data = try? JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys]) {
                try? data.write(to: URL(fileURLWithPath: reportPath), options: .atomic)
            }
            DispatchQueue.main.async { NSApp.terminate(nil) }
        }
    }
}
