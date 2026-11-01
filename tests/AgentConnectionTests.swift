import Foundation
import Darwin

// AgentConnection closes its socket only after the write queue lets go of it, so a reply that is
// still being written (peer not reading) can never land in an unrelated descriptor that reused the number.
@main struct AgentConnectionTests {
    static func pair() -> (Int32, Int32) {
        var fds: [Int32] = [0, 0]
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
        var on: Int32 = 1, small: Int32 = 4096
        for fd in fds { setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, socklen_t(MemoryLayout<Int32>.size)) }
        setsockopt(fds[0], SOL_SOCKET, SO_SNDBUF, &small, socklen_t(MemoryLayout<Int32>.size))
        _ = fcntl(fds[0], F_SETFL, fcntl(fds[0], F_GETFL) | O_NONBLOCK)
        return (fds[0], fds[1])
    }
    static func main() {
        let queue = DispatchQueue(label: "test.agent.io")
        var leaks = 0, checks = 0
        for _ in 0..<20 {
            let (fd, peer) = pair()
            let connection = AgentConnection(fd: fd, queue: queue)
            connection.start()
            connection.send(Data(repeating: 0x41, count: 1 << 20))   // the peer never reads: the writer spins on EAGAIN
            usleep(20_000)
            connection.close()
            // Grab the number the moment it is free; a writer still holding it would write into our socket.
            let deadline = Date().addingTimeInterval(2)
            while fcntl(fd, F_GETFD) != -1 && Date() < deadline { }
            var reused: [Int32] = [0, 0]
            precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &reused) == 0)
            _ = fcntl(reused[1], F_SETFL, O_NONBLOCK)
            usleep(30_000)
            var byte: UInt8 = 0
            if reused.contains(fd), read(reused[0] == fd ? reused[1] : reused[0], &byte, 1) > 0 { leaks += 1 }
            // After close, no further send may write anywhere.
            connection.send(Data("late".utf8)); usleep(5_000)
            if reused.contains(fd), read(reused[0] == fd ? reused[1] : reused[0], &byte, 1) > 0 { leaks += 1 }
            Darwin.close(reused[0]); Darwin.close(reused[1]); Darwin.close(peer)
            checks += 1
        }
        guard leaks == 0 else { print("FAIL AgentConnection wrote \(leaks) time(s) into a reused descriptor"); exit(1) }
        print("PASS \(checks) AgentConnection close/write ordering checks")
    }
}
