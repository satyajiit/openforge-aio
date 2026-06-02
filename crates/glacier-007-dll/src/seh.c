/*
 * SEH-guarded indirect call into Glacier engine functions.
 *
 * Rust cannot express Windows __try/__except, so this shim wraps a single
 * indirect call. The actuation entry points (SignalInputPin / SetPropertyValue
 * / SignalOutputPin) are standalone engine functions resolved by AOB; a stale
 * entity reference or a wrong-but-callable address would otherwise crash the
 * game on the access violation. Under __try, that fault becomes a clean -1
 * the caller turns into an Err instead of killing the process.
 *
 * One generic thunk covers the whole actuation surface. x64 MS ABI: there is a
 * single calling convention, so a0..a3 land in RCX/RDX/R8/R9 exactly as the
 * engine functions expect. ZEntityRef is an 8-byte value (= entity_va + 8)
 * passed in a register by value, so a uint64_t arg matches it precisely; a
 * `const ZObjectRef&` is a pointer, also a uint64_t. A bool return lands in AL
 * (the low byte of RAX) — callers mask it.
 *
 * Returns 0 on a completed call (RAX in *out_ret), -1 if a structured
 * exception fired. Compiled as plain C (no /EHa) so the __try intrinsic works.
 */

#include <windows.h>
#include <stdint.h>
#ifndef EXCEPTION_EXECUTE_HANDLER
#define EXCEPTION_EXECUTE_HANDLER 1
#endif

typedef uint64_t (*openforge_call4_fn)(uint64_t, uint64_t, uint64_t, uint64_t);

__declspec(dllexport) int32_t openforge_seh_call4(
    void* fn_ptr,
    uint64_t a0,
    uint64_t a1,
    uint64_t a2,
    uint64_t a3,
    uint64_t* out_ret,
    uint64_t* out_fault_rip,
    uint64_t* out_fault_addr,
    uint32_t* out_code)
{
    int32_t result = -1;
    EXCEPTION_POINTERS* ep = NULL;
    __try {
        openforge_call4_fn fn = (openforge_call4_fn)fn_ptr;
        uint64_t r = fn(a0, a1, a2, a3);
        if (out_ret != NULL) {
            *out_ret = r;
        }
        result = 0;
    }
    /* Capture the faulting instruction + access address so the caller can log
     * exactly where a bad call died (diagnosing thread/context vs null deref). */
    __except ((ep = GetExceptionInformation()), EXCEPTION_EXECUTE_HANDLER) {
        if (ep != NULL && ep->ExceptionRecord != NULL) {
            if (out_code != NULL) {
                *out_code = (uint32_t)ep->ExceptionRecord->ExceptionCode;
            }
            if (out_fault_rip != NULL) {
                *out_fault_rip = (uint64_t)ep->ExceptionRecord->ExceptionAddress;
            }
            if (out_fault_addr != NULL && ep->ExceptionRecord->NumberParameters >= 2) {
                *out_fault_addr = (uint64_t)ep->ExceptionRecord->ExceptionInformation[1];
            }
        }
        result = -1;
    }
    return result;
}
