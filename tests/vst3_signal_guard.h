#pragma once
// Diagnostic children only. Never include this in the production application.
// A fatal signal remains a failed test; normal success runs ordinary destructors.
#include <signal.h>
#include <unistd.h>

namespace VST3Diagnostic {
inline void fatalSignal(int signal) {
    static constexpr char segv[] = "SUPERTRACKER_DIAGNOSTIC_FATAL_SIGNAL=11\n";
    static constexpr char abrt[] = "SUPERTRACKER_DIAGNOSTIC_FATAL_SIGNAL=6\n";
    static constexpr char bus[] = "SUPERTRACKER_DIAGNOSTIC_FATAL_SIGNAL=10\n";
    if (signal == SIGSEGV) (void)write(STDERR_FILENO, segv, sizeof(segv) - 1);
    else if (signal == SIGABRT) (void)write(STDERR_FILENO, abrt, sizeof(abrt) - 1);
    else if (signal == SIGBUS) (void)write(STDERR_FILENO, bus, sizeof(bus) - 1);
    // POSIX _exit is async-signal-safe. This path is exclusively fatal and the
    // parent must classify 128+signal as failure, never as successful cleanup.
    _exit(128 + signal);
}
inline bool installFatalSignalGuard() {
    alignas(16) static unsigned char alternateStack[128 * 1024];
    stack_t stack{}; stack.ss_sp = alternateStack; stack.ss_size = sizeof(alternateStack);
    if (sigaltstack(&stack, nullptr) != 0) return false;
    struct sigaction action{}; action.sa_handler = fatalSignal;
    sigfillset(&action.sa_mask); action.sa_flags = SA_ONSTACK;
    return sigaction(SIGSEGV, &action, nullptr) == 0 &&
           sigaction(SIGABRT, &action, nullptr) == 0 &&
           sigaction(SIGBUS, &action, nullptr) == 0;
}
}
