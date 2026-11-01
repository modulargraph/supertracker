#include "vst3_signal_guard.h"
#include <cstdio>
#include <cstring>
#include <sys/mman.h>
struct NormalExit { ~NormalExit() { puts("NORMAL_DESTRUCTORS_RAN"); } } normalExit;
int main(int argc, char **argv) {
    if (!VST3Diagnostic::installFatalSignalGuard()) return 2;
    if (argc != 2) return 3;
    if (!strcmp(argv[1], "clean")) return 0;
    if (!strcmp(argv[1], "abrt")) raise(SIGABRT);
    if (!strcmp(argv[1], "bus")) raise(SIGBUS);
    if (!strcmp(argv[1], "segv")) raise(SIGSEGV);
    if (!strcmp(argv[1], "fault")) {
        void *page = mmap(nullptr, 4096, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
        if (page == MAP_FAILED) return 4;
        *static_cast<volatile char *>(page) = 1;
    }
    return 5;
}
