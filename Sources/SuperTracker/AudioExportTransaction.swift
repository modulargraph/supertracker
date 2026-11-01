import Foundation
import Darwin

/// Control-thread filesystem ownership for one export. Audio conversion/verification and Host
/// restoration must finish before publish(). Entries are private until one same-volume rename.
/// This guarantees atomic visibility, not power-loss durability or a global filesystem lock.
final class AudioExportTransaction {
    enum Kind { case file, stems }

    struct Entry {
        let sourceURL: URL
        /// Destination for AudioExportDelivery.Job; never give the Job finalURL.
        let destinationURL: URL
        let finalURL: URL
    }

    struct Failure: LocalizedError {
        let message: String
        var errorDescription: String? { message }
    }

    let destinationURL: URL
    let sourcesDirectoryURL: URL
    let deliveryDirectoryURL: URL
    /// A publication is successful even if subsequent private staging cleanup needs retrying.
    private(set) var cleanupError: String?

    private enum State { case open, published, discarded }
    private struct Identity: Equatable {
        let device: dev_t
        let inode: ino_t
        let mode: mode_t
        let size: off_t
        let modifiedSeconds: Int
        let modifiedNanos: Int
        let changedSeconds: Int
        let changedNanos: Int

        init(_ value: stat) {
            device = value.st_dev; inode = value.st_ino; mode = value.st_mode
            size = value.st_size
            modifiedSeconds = value.st_mtimespec.tv_sec
            modifiedNanos = value.st_mtimespec.tv_nsec
            changedSeconds = value.st_ctimespec.tv_sec
            changedNanos = value.st_ctimespec.tv_nsec
        }
        var type: mode_t { mode & S_IFMT }
        func sameObject(as other: Identity) -> Bool {
            device == other.device && inode == other.inode && type == other.type
        }
    }

    private let kind: Kind
    private let parentURL: URL
    private let destinationName: String
    private let privateName: String
    private let parentIdentity: Identity
    private let initialTarget: Identity?
    private let rootIdentity: Identity
    private let sourceIdentity: Identity
    private let deliveryIdentity: Identity
    private let parentFD: Int32
    private let rootFD: Int32
    private let sourceFD: Int32
    private let deliveryFD: Int32
    private var entries: [(name: String, value: Entry)] = []
    private var normalizedNames = Set<String>()
    private var state = State.open
    private var removedPrivateRoot = false

    init(destinationURL: URL, kind: Kind) throws {
        guard destinationURL.isFileURL, destinationURL.path.hasPrefix("/") else {
            throw Failure(message: "Choose a local absolute export destination.")
        }
        let destination = destinationURL.standardizedFileURL
        let name = destination.lastPathComponent
        try Self.validateLeaf(name)
        let parent = destination.deletingLastPathComponent()
        try Self.requireRealDirectoryPath(parent)
        let parentFD = open(parent.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard parentFD >= 0 else { throw Self.systemFailure("Open export folder") }
        var rootFD: Int32 = -1, sourceFD: Int32 = -1, deliveryFD: Int32 = -1
        let privateName = ".supertracker-export-\(UUID().uuidString).partial"
        var madeRoot = false
        do {
            let parentIdentity = try Self.identity(fd: parentFD)
            let target = try Self.identity(at: name, in: parentFD)
            switch kind {
            case .file:
                guard target == nil || target?.type == S_IFREG else {
                    throw Failure(message: "The export destination must be a regular file or a new filename.")
                }
            case .stems:
                guard target == nil || target?.type == S_IFDIR else {
                    throw Failure(message: "Stem export needs a new or empty folder.")
                }
                if target != nil { try Self.requireEmptyDirectory(name, in: parentFD) }
            }
            guard mkdirat(parentFD, privateName, 0o700) == 0 else {
                throw Self.systemFailure("Create private export staging folder")
            }
            madeRoot = true
            rootFD = openat(parentFD, privateName, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            guard rootFD >= 0 else { throw Self.systemFailure("Open private export staging folder") }
            guard mkdirat(rootFD, "sources", 0o700) == 0,
                  mkdirat(rootFD, "delivery", 0o700) == 0 else {
                throw Self.systemFailure("Create private export subfolders")
            }
            sourceFD = openat(rootFD, "sources", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            deliveryFD = openat(rootFD, "delivery", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            guard sourceFD >= 0, deliveryFD >= 0 else {
                throw Self.systemFailure("Open private export subfolders")
            }
            self.destinationURL = destination
            self.kind = kind
            self.parentURL = parent
            self.destinationName = name
            self.privateName = privateName
            self.parentIdentity = parentIdentity
            self.initialTarget = target
            self.rootIdentity = try Self.identity(fd: rootFD)
            self.sourceIdentity = try Self.identity(fd: sourceFD)
            self.deliveryIdentity = try Self.identity(fd: deliveryFD)
            self.parentFD = parentFD; self.rootFD = rootFD
            self.sourceFD = sourceFD; self.deliveryFD = deliveryFD
            let rootURL = parent.appendingPathComponent(privateName, isDirectory: true)
            sourcesDirectoryURL = rootURL.appendingPathComponent("sources", isDirectory: true)
            deliveryDirectoryURL = rootURL.appendingPathComponent("delivery", isDirectory: true)
        } catch {
            if rootFD >= 0 { try? Self.removeContents(in: rootFD) }
            if madeRoot { _ = unlinkat(parentFD, privateName, AT_REMOVEDIR) }
            for fd in [sourceFD, deliveryFD, rootFD, parentFD] where fd >= 0 { close(fd) }
            throw error
        }
    }

    deinit {
        try? removePrivateRoot()
        for fd in [sourceFD, deliveryFD, rootFD, parentFD] { close(fd) }
    }

    func register(filename: String) throws -> Entry {
        guard state == .open else { throw Failure(message: "This export transaction is already finished.") }
        try Self.validateLeaf(filename)
        let normalized = filename.precomposedStringWithCanonicalMapping.lowercased()
        guard !normalizedNames.contains(normalized) else {
            throw Failure(message: "Each exported stem needs a unique filename.")
        }
        guard kind == .stems || entries.isEmpty else {
            throw Failure(message: "A single-file export can register only one output.")
        }
        try requireOwnedDirectories()
        guard try Self.identity(at: filename, in: deliveryFD) == nil else {
            throw Failure(message: "The private export filename is already occupied.")
        }
        let value = Entry(
            sourceURL: sourcesDirectoryURL.appendingPathComponent(UUID().uuidString + ".caf"),
            destinationURL: deliveryDirectoryURL.appendingPathComponent(filename),
            finalURL: kind == .file ? destinationURL : destinationURL.appendingPathComponent(filename))
        entries.append((filename, value))
        normalizedNames.insert(normalized)
        return value
    }

    /// The caller has verified every delivery and restored the Host before this synchronous commit.
    /// Existing targets are checked immediately before rename; other processes are not locked out
    /// between that check and rename. Originally absent targets use an exclusive atomic rename.
    @discardableResult
    func publish() throws -> [URL] {
        guard state == .open else { throw Failure(message: "This export transaction is already finished.") }
        guard !entries.isEmpty else { throw Failure(message: "The export has no completed output files.") }
        try requireOwnedDirectories()
        let expectedNames = Set(entries.map(\.name))
        guard Set(try Self.names(in: deliveryFD)) == expectedNames else {
            throw Failure(message: "The export is incomplete or contains an unregistered delivery file.")
        }
        for entry in entries {
            guard let identity = try Self.identity(at: entry.name, in: deliveryFD),
                  identity.type == S_IFREG, identity.size > 0 else {
                throw Failure(message: "Every export delivery must be a nonempty regular file.")
            }
        }
        try Self.requireRealDirectoryPath(parentURL)
        guard let currentParent = try Self.identity(path: parentURL.path),
              parentIdentity.sameObject(as: currentParent) else {
            throw Failure(message: "The export parent folder changed. Choose the destination again.")
        }
        guard try Self.identity(at: destinationName, in: parentFD) == initialTarget else {
            throw Failure(message: "The export destination changed. Choose the destination again.")
        }
        if kind == .stems, initialTarget != nil {
            try Self.requireEmptyDirectory(destinationName, in: parentFD)
        }
        let fromFD = kind == .file ? deliveryFD : rootFD
        let fromName = kind == .file ? entries[0].name : "delivery"
        let result: Int32
        if initialTarget == nil {
            result = renameatx_np(fromFD, fromName, parentFD, destinationName, UInt32(RENAME_EXCL))
        } else {
            result = renameat(fromFD, fromName, parentFD, destinationName)
        }
        guard result == 0 else { throw Self.systemFailure("Publish completed export") }
        state = .published
        do { try removePrivateRoot(); cleanupError = nil }
        catch { cleanupError = error.localizedDescription }
        return entries.map { $0.value.finalURL }
    }

    /// Cancelling/failing an export removes only this transaction's private staging tree.
    /// Safe after publication too: published files are never inside that tree anymore.
    func discard() throws {
        if state == .open { state = .discarded }
        do { try removePrivateRoot(); cleanupError = nil }
        catch { cleanupError = error.localizedDescription; throw error }
    }

    private func requireOwnedDirectories() throws {
        guard let root = try Self.identity(at: privateName, in: parentFD),
              rootIdentity.sameObject(as: root),
              let source = try Self.identity(at: "sources", in: rootFD),
              sourceIdentity.sameObject(as: source),
              let delivery = try Self.identity(at: "delivery", in: rootFD),
              deliveryIdentity.sameObject(as: delivery) else {
            throw Failure(message: "The private export staging folder changed.")
        }
    }

    private func removePrivateRoot() throws {
        guard !removedPrivateRoot else { return }
        guard let root = try Self.identity(at: privateName, in: parentFD) else {
            removedPrivateRoot = true; return
        }
        guard rootIdentity.sameObject(as: root) else {
            throw Failure(message: "Private export cleanup refused a replaced staging folder.")
        }
        try Self.removeContents(in: rootFD)
        guard unlinkat(parentFD, privateName, AT_REMOVEDIR) == 0 else {
            throw Self.systemFailure("Remove private export staging folder")
        }
        removedPrivateRoot = true
    }

    private static func validateLeaf(_ name: String) throws {
        guard !name.isEmpty, name != ".", name != "..", name.utf8.count <= 255,
              !name.contains("/"), !name.contains("\\"), !name.contains(":"),
              !name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw Failure(message: "Export filenames must be single local names without path separators or control characters.")
        }
    }

    private static func requireRealDirectoryPath(_ url: URL) throws {
        var current = ""
        for component in url.pathComponents {
            current = component == "/" ? "/" : (current as NSString).appendingPathComponent(component)
            guard let item = try identity(path: current), item.type == S_IFDIR else {
                throw Failure(message: "The export parent must exist and cannot contain symbolic links.")
            }
        }
    }

    private static func identity(fd: Int32) throws -> Identity {
        var value = stat()
        guard fstat(fd, &value) == 0 else { throw systemFailure("Inspect export directory") }
        return Identity(value)
    }

    private static func identity(path: String) throws -> Identity? {
        var value = stat()
        if lstat(path, &value) == 0 { return Identity(value) }
        if errno == ENOENT { return nil }
        throw systemFailure("Inspect export path")
    }

    private static func identity(at name: String, in fd: Int32) throws -> Identity? {
        var value = stat()
        if fstatat(fd, name, &value, AT_SYMLINK_NOFOLLOW) == 0 { return Identity(value) }
        if errno == ENOENT { return nil }
        throw systemFailure("Inspect export entry")
    }

    private static func requireEmptyDirectory(_ name: String, in parent: Int32) throws {
        let fd = openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard fd >= 0 else { throw systemFailure("Open stem destination folder") }
        defer { close(fd) }
        guard try names(in: fd).isEmpty else {
            throw Failure(message: "Stem export cannot replace a nonempty folder. Choose a new or empty folder.")
        }
    }

    private static func names(in fd: Int32) throws -> [String] {
        // A new open file description avoids sharing readdir's offset with the retained descriptor.
        let copy = openat(fd, ".", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard copy >= 0 else { throw systemFailure("Read export folder") }
        guard let stream = fdopendir(copy) else { close(copy); throw systemFailure("Read export folder") }
        defer { closedir(stream) }
        var result: [String] = []
        while true {
            errno = 0
            guard let entry = readdir(stream) else {
                if errno != 0 { throw systemFailure("Read export folder") }
                return result
            }
            let name = withUnsafePointer(to: &entry.pointee.d_name) {
                $0.withMemoryRebound(to: CChar.self, capacity: Int(NAME_MAX) + 1) { String(cString: $0) }
            }
            if name != "." && name != ".." { result.append(name) }
        }
    }

    private static func removeContents(in fd: Int32) throws {
        for name in try names(in: fd) {
            guard let item = try identity(at: name, in: fd) else { continue }
            if item.type == S_IFDIR {
                let child = openat(fd, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
                guard child >= 0 else { throw systemFailure("Open private export cleanup folder") }
                defer { close(child) }
                guard try item.sameObject(as: identity(fd: child)) else {
                    throw Failure(message: "Private export cleanup found a replaced subfolder.")
                }
                try removeContents(in: child)
                guard unlinkat(fd, name, AT_REMOVEDIR) == 0 else { throw systemFailure("Remove private export subfolder") }
            } else {
                // Includes symlinks: unlink the private entry itself, never follow it.
                guard unlinkat(fd, name, 0) == 0 else { throw systemFailure("Remove private export file") }
            }
        }
    }

    private static func systemFailure(_ operation: String) -> Failure {
        let code = errno
        return Failure(message: "\(operation): \(String(cString: strerror(code))) (\(code)).")
    }
}
