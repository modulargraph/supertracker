// The actual Driver is extracted verbatim by test-mk3-lifecycle.sh; only device/NI I/O is replaced.
import Foundation
@main enum MaschineMK3LifecycleTests {
    @MainActor static func entered() async -> Bool {
        for _ in 0..<2000 {
            if MK3AgentController.hasEntered() { return true }
            try? await Task.sleep(nanoseconds: 1_000_000)
        }
        return false
    }
    @MainActor static func yieldCalls() async { try? await Task.sleep(nanoseconds: 40_000_000) }
    @MainActor static func main() async {
        let scenario = CommandLine.arguments.dropFirst().first ?? "release"
        let driver = MaschineMK3Driver.shared
        var valid = false
        if scenario == "transfer-validation" {
            var rejected=0
            for pair in [(100,0),(100,99),(100,101),(0,0),(100,-1)] {
                do { try MK3ScreenTransferValidation.requireComplete(requested:pair.0,transferred:pair.1) }
                catch { rejected += 1 }
            }
            do { try MK3ScreenTransferValidation.requireComplete(requested:100,transferred:100);valid=rejected == 5 }
            catch { valid=false }
        } else if scenario == "missing-device" {
            MK3ScreenSession.present=false
            let reply=await driver.connect(exclusive:true)
            let original=reply["error"] as? String
            MK3HIDSession.reconnect();await yieldCalls()
            valid = !driver.screens && !driver.exclusive && MK3AgentController.stopCalls == 0 && MK3AgentController.opens == 0 &&
                original?.contains("Connect and power on") == true && driver.lastError == original
            MK3ScreenSession.present=true;MK3AgentController.gate.signal()
            _ = await driver.connect(exclusive:true)
            valid = valid && driver.screens && driver.exclusive && driver.lastError == nil && MK3AgentController.stopCalls == 1
            _ = await driver.disconnect()
            valid = valid && MK3AgentController.restoreCalls == 1
        } else if scenario == "open-retry" || scenario == "initialization-retry" {
            if scenario == "open-retry" { MK3ScreenSession.openFailures=1 }
            else { MK3ScreenSession.initializeFailures=1 }
            MK3AgentController.gate.signal()
            let reply=await driver.connect(exclusive:true)
            let original=reply["error"] as? String
            MK3HIDSession.reconnect();await yieldCalls()
            valid = !driver.screens && !driver.exclusive && original?.contains("fixture") == true && driver.lastError == original &&
                MK3AgentController.restoreCalls == 1 && (MK3AgentController.readState() ?? []).isEmpty
            MK3AgentController.gate.signal()
            _ = await driver.connect(exclusive:true)
            valid = valid && driver.screens && driver.exclusive && driver.lastError == nil && MK3AgentController.opens == 2
            _ = await driver.disconnect()
            valid = valid && MK3AgentController.restoreCalls == 2
        } else if scenario == "shared-shutdown" {
            _ = await driver.connect(exclusive: false)
            // An unrelated durable record must not make a shared-only shutdown restore agents.
            MK3AgentController.recorded = [MK3AgentRecord()]
            driver.shutdownSynchronously(); await yieldCalls()
            valid = !driver.running && !driver.connected && MK3AgentController.stopCalls == 0 && MK3AgentController.restoreCalls == 0
        } else if scenario == "shutdown-release" {
            MK3AgentController.gate.signal(); _ = await driver.connect(exclusive: true)
            MK3ScreenSession.blockClose = true
            let releasing = Task { await driver.disconnect() }
            for _ in 0..<2000 {
                if MK3ScreenSession.hasEnteredClose() { break }
                try? await Task.sleep(nanoseconds: 1_000_000)
            }
            guard MK3ScreenSession.hasEnteredClose() else { print("FAIL fixture did not suspend screen close"); exit(2) }
            DispatchQueue.global().asyncAfter(deadline: .now() + 0.04) { MK3ScreenSession.closeGate.signal() }
            driver.shutdownSynchronously(); _ = await releasing.value; await yieldCalls()
            valid = !driver.running && !driver.exclusive && !driver.screens && MK3AgentController.restoreCalls == 1
        } else if scenario == "restore-retry" {
            MK3AgentController.gate.signal()
            _ = await driver.connect(exclusive: true)
            MK3AgentController.restoreFailures = 1
            _ = await driver.disconnect()
            _ = await driver.disconnect()
            valid = !driver.exclusive && MK3AgentController.restoreCalls == 2 && (MK3AgentController.readState() ?? []).isEmpty
        } else {
            let connecting = Task { await driver.connect(exclusive: true) }
            guard await entered() else { print("FAIL fixture did not suspend takeover"); exit(2) }
            if scenario == "duplicate" {
                let again = Task { await driver.connect(exclusive: true) }
                await yieldCalls(); MK3AgentController.gate.signal()
                _ = await connecting.value; _ = await again.value
                valid = driver.exclusive && driver.screens && MK3AgentController.stopCalls == 1 && MK3AgentController.opens == 1
                _ = await driver.disconnect()
                valid = valid && MK3AgentController.restoreCalls == 1
            } else if scenario == "shutdown" || scenario == "shutdown-queued" {
                let queued: Task<[String: Any], Never>? = scenario == "shutdown-queued" ? Task { await driver.connect(exclusive: true) } : nil
                await yieldCalls()
                DispatchQueue.global().asyncAfter(deadline: .now() + 0.04) { MK3AgentController.gate.signal() }
                driver.shutdownSynchronously()
                _ = await connecting.value; _ = await queued?.value; await yieldCalls()
                valid = !driver.running && !driver.connected && !driver.exclusive && !driver.screens && MK3AgentController.stopCalls == 1 && MK3AgentController.restoreCalls == 1 && MK3AgentController.opens == 0
            } else {
                let disconnecting = Task { await driver.disconnect() }
                await yieldCalls()
                let reconnecting: Task<[String: Any], Never>? = scenario == "reconnect" ? Task { await driver.connect(exclusive: false) } : nil
                await yieldCalls(); MK3AgentController.gate.signal()
                _ = await connecting.value; _ = await disconnecting.value; _ = await reconnecting?.value
                valid = !driver.exclusive && !driver.screens && driver.running == (scenario == "reconnect") && MK3AgentController.stopCalls == 1 && MK3AgentController.restoreCalls == 1
            }
        }
        print("\(valid ? "PASS" : "FAIL") \(scenario): status=\(driver.status); stops=\(MK3AgentController.stopCalls), restores=\(MK3AgentController.restoreCalls), opens=\(MK3AgentController.opens)")
        driver.shutdownSynchronously()
        exit(valid ? 0 : 1)
    }
}
