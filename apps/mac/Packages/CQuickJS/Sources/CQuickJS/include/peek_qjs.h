/*
 * peek_qjs.h: the only QuickJS API Swift calls (BLUEPRINT §8.1, visual.md B3).
 *
 * QuickJS's JSValue is a struct built by compound-literal macros that Swift
 * cannot import, so every JSValue, refcount and exception stays in C. Swift
 * sees one opaque PeekVM per drawing and a handful of calls.
 *
 * Contract with the JavaScript prelude (owned by PeekDrawing):
 *
 *   Globals the shim installs before any script runs:
 *     __peek_ops        Float64Array view over the VM's native op buffer
 *                       (op_capacity doubles; zero-copy).
 *     __peek_flush(len, strings, again)
 *                       Publishes a frame: `len` doubles of __peek_ops, an array
 *                       of strings referenced by index from the op stream, and
 *                       whether the drawing wants another frame. Call it once
 *                       per __peek_frame; a later call in the same frame wins.
 *     __peek_log(message)
 *                       Forwards one already-formatted string to the log
 *                       callback in PeekVMConfig.
 *     __peek_measure(font, text)
 *                       Returns an array of PEEK_TEXT_METRICS_COUNT numbers
 *                       (see PeekTextMetric) measured by the measure callback
 *                       in PeekVMConfig, for ctx.measureText. Without a
 *                       callback every metric is 0.
 *
 *   Globals the prelude must define, then bind with peek_vm_bind_entry_points():
 *     __peek_frame(input)          required; `input` is the parsed JSON snapshot
 *     __peek_event(name, payload)  optional; one-off events (enter, leave, ...)
 *
 * Threading: a PeekVM is not thread-safe. Use it from one thread at a time
 * (visual.md B4: one dedicated Thread per drawing). The shim calls
 * JS_UpdateStackTop on every entry, so the owning thread may change between
 * calls, but the thread's stack must be larger than max_stack_size.
 * peek_vm_request_abort() is the only call that may run on another thread.
 *
 * Deadlines are absolute peek_monotonic_ns() values (CLOCK_UPTIME_RAW); 0 means
 * no deadline. QuickJS polls the interrupt handler every 10,000 poll points
 * (calls and backward jumps), and the interrupt is uncatchable by the script.
 */
#ifndef PEEK_QJS_H
#define PEEK_QJS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Defaults applied when a PeekVMConfig field is 0. */
#define PEEK_DEFAULT_MEMORY_LIMIT ((size_t)16 << 20)   /* visual.md A7: 16 MB */
#define PEEK_DEFAULT_MAX_STACK_SIZE ((size_t)1 << 20)  /* needs a >= 4 MiB thread stack */
#define PEEK_DEFAULT_OP_CAPACITY ((size_t)1 << 16)     /* 65,536 doubles = 512 KiB */
#define PEEK_MAX_STRING_BYTES ((size_t)1 << 20)        /* per flush, all strings together */

typedef struct PeekVM PeekVM;

typedef enum PeekStatus {
    PEEK_STATUS_OK = 0,
    /* The script threw. peek_vm_error_message/peek_vm_error_stack describe it. */
    PEEK_STATUS_THREW = 1,
    /* The deadline passed or peek_vm_request_abort() was called. */
    PEEK_STATUS_INTERRUPTED = 2,
    /* The runtime hit its memory limit. Destroy the VM (visual.md B10). */
    PEEK_STATUS_OUT_OF_MEMORY = 3,
    /* __peek_frame is not bound (see peek_vm_bind_entry_points). */
    PEEK_STATUS_MISSING_ENTRY = 4,
    /* NULL VM, NULL required pointer, or an input that is not valid JSON. */
    PEEK_STATUS_INVALID_ARGUMENT = 5
} PeekStatus;

/* Receives one peek.log line as UTF-8 (not NUL-terminated). Called on the VM's thread. */
typedef void (*PeekLogCallback)(void *opaque, const char *utf8, size_t len);

/* Indices into the metrics array filled by PeekMeasureCallback (drawing units). */
typedef enum PeekTextMetric {
    PEEK_TEXT_METRIC_WIDTH = 0,
    PEEK_TEXT_METRIC_ACTUAL_LEFT = 1,
    PEEK_TEXT_METRIC_ACTUAL_RIGHT = 2,
    PEEK_TEXT_METRIC_ACTUAL_ASCENT = 3,
    PEEK_TEXT_METRIC_ACTUAL_DESCENT = 4,
    PEEK_TEXT_METRIC_FONT_ASCENT = 5,
    PEEK_TEXT_METRIC_FONT_DESCENT = 6,
    PEEK_TEXT_METRICS_COUNT = 7
} PeekTextMetric;

/* Measures `text` (UTF-8) set in the CSS font shorthand `font` (UTF-8), neither
 * NUL-terminated, and writes PEEK_TEXT_METRICS_COUNT values into `metrics`
 * (pre-zeroed). Called on the VM's thread, inside a script call. */
typedef void (*PeekMeasureCallback)(void *opaque, const char *font, size_t font_len, const char *text,
                                    size_t text_len, double *metrics);

typedef struct PeekVMConfig {
    size_t memory_limit;         /* bytes; 0 = PEEK_DEFAULT_MEMORY_LIMIT */
    size_t max_stack_size;       /* bytes; 0 = PEEK_DEFAULT_MAX_STACK_SIZE */
    size_t op_capacity;          /* doubles in __peek_ops; 0 = PEEK_DEFAULT_OP_CAPACITY */
    PeekLogCallback log;         /* optional */
    void *log_opaque;            /* passed back to `log` */
    PeekMeasureCallback measure; /* optional; backs __peek_measure */
    void *measure_opaque;        /* passed back to `measure` */
} PeekVMConfig;

/* The frame published by __peek_flush. Every pointer is owned by the VM and
 * stays valid until the next peek_vm_* call on the same VM. */
typedef struct PeekFrameOutput {
    bool flushed;                   /* __peek_flush ran during this call */
    bool again;                     /* the drawing asked for another frame */
    const double *ops;              /* op_count doubles */
    size_t op_count;
    const char *string_bytes;       /* all strings, UTF-8, back to back */
    const uint32_t *string_offsets; /* string i = string_bytes[offsets[i] ..< offsets[i] + lengths[i]] */
    const uint32_t *string_lengths;
    size_t string_count;
} PeekFrameOutput;

/* Creates a runtime + context with only BaseObjects, Eval, RegExp, JSON, MapSet,
 * TypedArrays and Promise (BLUEPRINT §8.3). No std/os modules, no module loader,
 * no timers. Returns NULL when allocation fails. `config` may be NULL. */
PeekVM *peek_vm_new(const PeekVMConfig *config);

/* Frees the context, the runtime and the native buffers. NULL is ignored. */
void peek_vm_free(PeekVM *vm);

/* Evaluates a classic script in the global scope. `source` need not be
 * NUL-terminated. `strict` forces strict mode. */
PeekStatus peek_vm_eval(PeekVM *vm, const char *source, size_t len, const char *filename,
                        bool strict, uint64_t deadline_ns);

/* Looks up __peek_frame (required) and __peek_event (optional) and keeps
 * references to them. When `hide_globals` is true it also deletes __peek_frame,
 * __peek_event, __peek_ops, __peek_flush, __peek_log and __peek_measure from the
 * global object so a drawing cannot replace or call them. Call once, after the
 * prelude. */
PeekStatus peek_vm_bind_entry_points(PeekVM *vm, bool hide_globals);

/* Parses `input_json` (need not be NUL-terminated) and calls __peek_frame(input).
 * Pending promise jobs run afterwards under the same deadline. `out` is always
 * written; out->flushed tells whether __peek_flush ran. */
PeekStatus peek_vm_frame(PeekVM *vm, const char *input_json, size_t len, uint64_t deadline_ns,
                         PeekFrameOutput *out);

/* Calls __peek_event(name, payload). `payload_json` may be NULL (payload is
 * undefined). Returns PEEK_STATUS_OK without calling anything when no
 * __peek_event was bound. */
PeekStatus peek_vm_event(PeekVM *vm, const char *name, const char *payload_json, size_t len,
                         uint64_t deadline_ns);

/* Last error of a call that returned THREW, INTERRUPTED or OUT_OF_MEMORY.
 * UTF-8, owned by the VM, valid until the next failing call. Never NULL
 * (empty string when there is none); `len` may be NULL. */
const char *peek_vm_error_message(const PeekVM *vm, size_t *len);
const char *peek_vm_error_stack(const PeekVM *vm, size_t *len);

/* Thread-safe. Makes the running (and every later) call return
 * PEEK_STATUS_INTERRUPTED until peek_vm_clear_abort(). Used for teardown. */
void peek_vm_request_abort(PeekVM *vm);
void peek_vm_clear_abort(PeekVM *vm);

/* Bytes currently used by the JS heap (JS_ComputeMemoryUsage.memory_used_size). */
size_t peek_vm_memory_used(PeekVM *vm);

/* Runs the cycle collector. */
void peek_vm_run_gc(PeekVM *vm);

/* The op buffer capacity in doubles, as configured. */
size_t peek_vm_op_capacity(const PeekVM *vm);

/* CLOCK_UPTIME_RAW in nanoseconds: the clock deadlines are measured against. */
uint64_t peek_monotonic_ns(void);

/* "0.17.0": the vendored quickjs-ng version. */
const char *peek_qjs_version(void);

#ifdef __cplusplus
}
#endif

#endif /* PEEK_QJS_H */
