import Darwin

// OSAtomic is available on the macOS 14 deployment target. It is used only for pointer
// publication and small transport values, never for a spin-lock on the render thread.
func atomicLoad(_ pointer: UnsafeMutablePointer<Int64>) -> Int64 { OSAtomicAdd64Barrier(0, pointer) }
func atomicStore(_ value: Int64, at pointer: UnsafeMutablePointer<Int64>) {
    var old = atomicLoad(pointer)
    while !OSAtomicCompareAndSwap64Barrier(old, value, pointer) { old = atomicLoad(pointer) }
}
