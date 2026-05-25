/*
 * Structured Exception Handling wrapper for calls into engine code.
 *
 * Rust does not expose Windows __try/__except, so this C shim wraps both
 * the call AND the subsequent read of the FString out-param. A wrong-but-
 * not-immediately-crashing function may leave FString.data pointing to
 * garbage memory; reading from it outside SEH protection would crash the
 * process. Doing the full call+read sequence under one __try block makes
 * the probe truly safe to retry across many candidates.
 *
 * Same pattern UE4SS uses (SEH_TRY / SEH_EXCEPT macros).
 *
 * Returns the number of UTF-16 wide chars written into `out_chars` (NOT
 * including the trailing NUL we add) on success, or -1 if any structured
 * exception fired during the call+read. `out_chars` is a wide-char buffer
 * of `out_chars_cap` elements; the resulting string is truncated to fit.
 */

#include <windows.h>
#include <stdint.h>

typedef void (*fname_to_string_fn)(void* fname, void* out_fstring);

/* In-memory layout of UE5 FString: { wchar_t* data; int32_t num; int32_t max; } */
struct FStringRepr {
    uint16_t* data;
    int32_t num;
    int32_t max;
};

__declspec(dllexport) int32_t openforge_seh_call_fname_to_string(
    fname_to_string_fn fn_ptr,
    void* fname,
    uint16_t* out_chars,
    int32_t out_chars_cap)
{
    int32_t result = -1;
    __try {
        struct FStringRepr local = { 0, 0, 0 };
        fn_ptr(fname, &local);

        if (local.data == NULL) {
            result = -1;
        } else if (local.num < 1 || local.num > 256) {
            /* Implausible length for an FName ("None" is 5 incl NUL) */
            result = -1;
        } else {
            int32_t take = local.num - 1; /* strip trailing NUL */
            if (take < 0) take = 0;
            if (take >= out_chars_cap) take = out_chars_cap - 1;
            /* Read inside __try so a bad data pointer doesn't escape */
            for (int32_t i = 0; i < take; ++i) {
                out_chars[i] = local.data[i];
            }
            out_chars[take] = 0;
            result = take;
        }
    }
    __except (EXCEPTION_EXECUTE_HANDLER) {
        result = -1;
    }
    return result;
}

/*
 * SEH wrapper for UE5's UObject::ProcessEvent. Signature:
 *     void(*)(UObject* obj, UFunction* func, void* parms_buf)
 *
 * Why this exists: ProcessEvent is the universal UFunction dispatcher.
 * Callers (the Lua runtime in particular, but also CLI / app-side
 * trainer features) may pass an obj_addr or function pointer that's
 * been freed by the engine between FindUObject and CallUFunction
 * (typical pattern: world transition invalidates Pawn pointers).
 *
 * Catching it under __try means a stale-pointer access violation
 * becomes a clean Err in Rust instead of a process crash. Same
 * pattern UE4SS uses for its own dynamic UFunction dispatch.
 *
 * Returns 0 on a successful call, -1 on any structured exception
 * (access violation, illegal instruction, stack overflow, etc.).
 * The parms_buf may have been partially mutated by the engine before
 * the fault — that's acceptable: callers either ignore the buffer on
 * error (current behavior) or treat partial data as garbage.
 */
typedef void (*process_event_fn)(void* obj, void* func, void* parms);

__declspec(dllexport) int32_t openforge_seh_call_process_event(
    process_event_fn fn_ptr,
    void* obj,
    void* func,
    void* parms)
{
    int32_t result = -1;
    __try {
        fn_ptr(obj, func, parms);
        result = 0;
    }
    __except (EXCEPTION_EXECUTE_HANDLER) {
        result = -1;
    }
    return result;
}
