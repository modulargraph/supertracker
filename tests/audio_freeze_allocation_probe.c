#include <stdint.h>
#include <stddef.h>

/* Test-only Apple dyld interposition. Counters cover Swift object/raw heap
   allocation, not arbitrary system allocator activity elsewhere in the process. */
extern void *swift_allocObject(const void *, size_t, size_t);
extern void *swift_slowAlloc(size_t, size_t);
extern void swift_deallocObject(void *, size_t, size_t);
extern void swift_slowDealloc(void *, size_t, size_t);
extern void swift_deallocClassInstance(void *, size_t, size_t);
extern void swift_deallocPartialClassInstance(void *, const void *, size_t, size_t);
static _Thread_local int enabled;
static _Thread_local uint64_t allocations, deallocations;
void freeze_probe_begin(void) { allocations = 0; deallocations = 0; enabled = 1; }
void freeze_probe_end(void) { enabled = 0; }
uint64_t freeze_probe_allocations(void) { return allocations; }
uint64_t freeze_probe_deallocations(void) { return deallocations; }
static void *probe_object(const void *metadata, size_t size, size_t alignment) {
    if (enabled) ++allocations;
    return swift_allocObject(metadata, size, alignment);
}
static void *probe_raw(size_t size, size_t alignment) {
    if (enabled) ++allocations;
    return swift_slowAlloc(size, alignment);
}
static void probe_release_object(void *object, size_t size, size_t alignment) {
    if (enabled) ++deallocations;
    swift_deallocObject(object, size, alignment);
}
static void probe_release_raw(void *object, size_t size, size_t alignment) {
    if (enabled) ++deallocations;
    swift_slowDealloc(object, size, alignment);
}
static void probe_release_class(void *object, size_t size, size_t alignment) {
    if (enabled) ++deallocations;
    swift_deallocClassInstance(object, size, alignment);
}
static void probe_release_partial_class(void *object, const void *metadata, size_t size, size_t alignment) {
    if (enabled) ++deallocations;
    swift_deallocPartialClassInstance(object, metadata, size, alignment);
}
#define INTERPOSE(replacement, original) \
    __attribute__((used)) static struct { const void *replacement; const void *original; } \
    interpose_##original __attribute__((section("__DATA,__interpose"))) = \
    { (const void *)(uintptr_t)&replacement, (const void *)(uintptr_t)&original }
INTERPOSE(probe_object, swift_allocObject);
INTERPOSE(probe_raw, swift_slowAlloc);
INTERPOSE(probe_release_object, swift_deallocObject);
INTERPOSE(probe_release_raw, swift_slowDealloc);
INTERPOSE(probe_release_class, swift_deallocClassInstance);
INTERPOSE(probe_release_partial_class, swift_deallocPartialClassInstance);
