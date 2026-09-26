/*
 * peek_qjs.c: the C shim between Swift and quickjs-ng. See include/peek_qjs.h
 * for the contract. Every JSValue, refcount and exception is handled here.
 */
#include "peek_qjs.h"

#include "quickjs.h"

#include <math.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

struct PeekVM {
    JSRuntime *rt;
    JSContext *ctx;

    double *ops;
    size_t op_capacity;

    JSValue frame_fn; /* JS_UNDEFINED until peek_vm_bind_entry_points */
    JSValue event_fn; /* JS_UNDEFINED when the prelude defines no __peek_event */
    bool bound;

    PeekLogCallback log;
    void *log_opaque;
    PeekMeasureCallback measure;
    void *measure_opaque;

    uint64_t deadline_ns;
    atomic_bool abort_requested;
    bool interrupt_fired;

    /* State published by __peek_flush. */
    bool flushed;
    bool again;
    size_t op_count;
    char *string_bytes;
    size_t string_bytes_len;
    size_t string_bytes_cap;
    uint32_t *string_offsets;
    uint32_t *string_lengths;
    size_t string_count;
    size_t string_cap;

    /* NUL-terminated copy of the last source or JSON input (QuickJS requires it). */
    char *scratch;
    size_t scratch_cap;

    char *error_message;
    size_t error_message_len;
    char *error_stack;
    size_t error_stack_len;
};

static const char peek_empty_string[] = "";

uint64_t peek_monotonic_ns(void) { return clock_gettime_nsec_np(CLOCK_UPTIME_RAW); }

const char *peek_qjs_version(void) { return JS_GetVersion(); }

static int peek_interrupt_handler(JSRuntime *rt, void *opaque) {
    (void)rt;
    PeekVM *vm = (PeekVM *)opaque;
    if (atomic_load_explicit(&vm->abort_requested, memory_order_relaxed)) {
        vm->interrupt_fired = true;
        return 1;
    }
    if (vm->deadline_ns != 0 && peek_monotonic_ns() > vm->deadline_ns) {
        vm->interrupt_fired = true;
        return 1;
    }
    return 0;
}

/* ---- error capture ---------------------------------------------------- */

static void peek_set_owned_string(char **slot, size_t *slot_len, const char *src, size_t len) {
    free(*slot);
    *slot = NULL;
    *slot_len = 0;
    if (src == NULL) {
        return;
    }
    char *copy = (char *)malloc(len + 1);
    if (copy == NULL) {
        return;
    }
    memcpy(copy, src, len);
    copy[len] = '\0';
    *slot = copy;
    *slot_len = len;
}

static void peek_clear_error(PeekVM *vm) {
    peek_set_owned_string(&vm->error_message, &vm->error_message_len, NULL, 0);
    peek_set_owned_string(&vm->error_stack, &vm->error_stack_len, NULL, 0);
}

static void peek_set_error_literal(PeekVM *vm, const char *message) {
    peek_set_owned_string(&vm->error_message, &vm->error_message_len, message, strlen(message));
    peek_set_owned_string(&vm->error_stack, &vm->error_stack_len, NULL, 0);
}

/* Copies a JS value's string form into an owned slot. Returns false when the
 * conversion itself failed (and clears the resulting exception). */
static bool peek_copy_js_string(PeekVM *vm, JSValueConst value, char **slot, size_t *slot_len) {
    size_t len = 0;
    const char *str = JS_ToCStringLen(vm->ctx, &len, value);
    if (str == NULL) {
        JSValue nested = JS_GetException(vm->ctx);
        JS_FreeValue(vm->ctx, nested);
        return false;
    }
    peek_set_owned_string(slot, slot_len, str, len);
    JS_FreeCString(vm->ctx, str);
    return true;
}

static bool peek_message_is_oom(const char *message) {
    if (message == NULL) {
        return false;
    }
    return strcmp(message, "InternalError: out of memory") == 0 ||
           strcmp(message, "InternalError: out of memory in regexp execution") == 0;
}

/* Takes the pending exception, classifies it and records message + stack. */
static PeekStatus peek_capture_exception(PeekVM *vm) {
    JSContext *ctx = vm->ctx;
    JSValue exc = JS_GetException(ctx);
    bool uncatchable = JS_IsUncatchableError(exc);
    bool interrupted = vm->interrupt_fired || uncatchable;

    peek_clear_error(vm);
    if (JS_IsNull(exc)) {
        /* QuickJS throws null when it cannot even allocate the error object. */
        JS_FreeValue(ctx, exc);
        peek_set_error_literal(vm, "InternalError: out of memory");
        JS_RunGC(vm->rt);
        return PEEK_STATUS_OUT_OF_MEMORY;
    }

    if (!peek_copy_js_string(vm, exc, &vm->error_message, &vm->error_message_len)) {
        peek_set_owned_string(&vm->error_message, &vm->error_message_len,
                              "exception could not be converted to a string",
                              strlen("exception could not be converted to a string"));
    }
    if (JS_IsObject(exc)) {
        JSValue stack = JS_GetPropertyStr(ctx, exc, "stack");
        if (JS_IsException(stack)) {
            JSValue nested = JS_GetException(ctx);
            JS_FreeValue(ctx, nested);
        } else if (JS_IsString(stack)) {
            peek_copy_js_string(vm, stack, &vm->error_stack, &vm->error_stack_len);
        }
        JS_FreeValue(ctx, stack);
    }
    JS_FreeValue(ctx, exc);

    if (interrupted) {
        return PEEK_STATUS_INTERRUPTED;
    }
    if (peek_message_is_oom(vm->error_message)) {
        JS_RunGC(vm->rt);
        return PEEK_STATUS_OUT_OF_MEMORY;
    }
    return PEEK_STATUS_THREW;
}

/* ---- call bracketing -------------------------------------------------- */

static void peek_begin_call(PeekVM *vm, uint64_t deadline_ns) {
    JS_UpdateStackTop(vm->rt);
    vm->deadline_ns = deadline_ns;
    vm->interrupt_fired = false;
}

static void peek_end_call(PeekVM *vm) { vm->deadline_ns = 0; }

/* Runs queued promise jobs until none is left or one fails. */
static PeekStatus peek_run_pending_jobs(PeekVM *vm) {
    for (;;) {
        JSContext *job_ctx = NULL;
        int ret = JS_ExecutePendingJob(vm->rt, &job_ctx);
        if (ret == 0) {
            return PEEK_STATUS_OK;
        }
        if (ret < 0) {
            return peek_capture_exception(vm);
        }
    }
}

/* Copies `len` bytes into the NUL-terminated scratch buffer. */
static const char *peek_scratch_copy(PeekVM *vm, const char *src, size_t len) {
    if (len == SIZE_MAX) {
        return NULL;
    }
    if (vm->scratch_cap < len + 1) {
        size_t cap = vm->scratch_cap ? vm->scratch_cap : 4096;
        while (cap < len + 1) {
            if (cap > SIZE_MAX / 2) {
                cap = len + 1;
                break;
            }
            cap *= 2;
        }
        char *grown = (char *)realloc(vm->scratch, cap);
        if (grown == NULL) {
            return NULL;
        }
        vm->scratch = grown;
        vm->scratch_cap = cap;
    }
    if (len > 0) {
        memcpy(vm->scratch, src, len);
    }
    vm->scratch[len] = '\0';
    return vm->scratch;
}

/* ---- native globals ----------------------------------------------------- */

static bool peek_reserve_strings(PeekVM *vm, size_t count) {
    if (count <= vm->string_cap) {
        return true;
    }
    size_t cap = vm->string_cap ? vm->string_cap : 64;
    while (cap < count) {
        cap *= 2;
    }
    uint32_t *offsets = (uint32_t *)realloc(vm->string_offsets, cap * sizeof(uint32_t));
    if (offsets == NULL) {
        return false;
    }
    vm->string_offsets = offsets;
    uint32_t *lengths = (uint32_t *)realloc(vm->string_lengths, cap * sizeof(uint32_t));
    if (lengths == NULL) {
        return false;
    }
    vm->string_lengths = lengths;
    vm->string_cap = cap;
    return true;
}

static bool peek_append_string_bytes(PeekVM *vm, const char *bytes, size_t len) {
    size_t needed = vm->string_bytes_len + len;
    if (needed > vm->string_bytes_cap) {
        size_t cap = vm->string_bytes_cap ? vm->string_bytes_cap : 4096;
        while (cap < needed) {
            cap *= 2;
        }
        char *grown = (char *)realloc(vm->string_bytes, cap);
        if (grown == NULL) {
            return false;
        }
        vm->string_bytes = grown;
        vm->string_bytes_cap = cap;
    }
    if (len > 0) {
        memcpy(vm->string_bytes + vm->string_bytes_len, bytes, len);
    }
    vm->string_bytes_len = needed;
    return true;
}

static JSValue peek_js_flush(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv) {
    (void)this_val;
    PeekVM *vm = (PeekVM *)JS_GetContextOpaque(ctx);
    if (argc < 3) {
        return JS_ThrowTypeError(ctx, "__peek_flush(len, strings, again) expects 3 arguments, got %d", argc);
    }

    double len_d = 0;
    if (JS_ToFloat64(ctx, &len_d, argv[0]) != 0) {
        return JS_EXCEPTION;
    }
    if (!(len_d >= 0) || len_d > (double)vm->op_capacity || len_d != floor(len_d)) {
        return JS_ThrowRangeError(ctx, "__peek_flush: op count %g is not an integer in 0...%zu", len_d,
                                  vm->op_capacity);
    }

    vm->flushed = false;
    vm->string_count = 0;
    vm->string_bytes_len = 0;

    JSValueConst strings = argv[1];
    if (!JS_IsUndefined(strings) && !JS_IsNull(strings)) {
        if (!JS_IsArray(strings)) {
            return JS_ThrowTypeError(ctx, "__peek_flush: strings must be an array of strings");
        }
        int64_t count = 0;
        if (JS_GetLength(ctx, strings, &count) != 0) {
            return JS_EXCEPTION;
        }
        if (count < 0 || (uint64_t)count > (uint64_t)vm->op_capacity) {
            return JS_ThrowRangeError(ctx, "__peek_flush: %lld strings exceed the op capacity %zu",
                                      (long long)count, vm->op_capacity);
        }
        if (!peek_reserve_strings(vm, (size_t)count)) {
            return JS_ThrowOutOfMemory(ctx);
        }
        for (int64_t i = 0; i < count; i++) {
            JSValue item = JS_GetPropertyUint32(ctx, strings, (uint32_t)i);
            if (JS_IsException(item)) {
                return JS_EXCEPTION;
            }
            size_t len = 0;
            const char *str = JS_ToCStringLen(ctx, &len, item);
            JS_FreeValue(ctx, item);
            if (str == NULL) {
                return JS_EXCEPTION;
            }
            if (vm->string_bytes_len + len > PEEK_MAX_STRING_BYTES) {
                JS_FreeCString(ctx, str);
                return JS_ThrowRangeError(ctx, "__peek_flush: strings exceed %zu bytes in one frame",
                                          (size_t)PEEK_MAX_STRING_BYTES);
            }
            uint32_t offset = (uint32_t)vm->string_bytes_len;
            bool ok = peek_append_string_bytes(vm, str, len);
            JS_FreeCString(ctx, str);
            if (!ok) {
                return JS_ThrowOutOfMemory(ctx);
            }
            vm->string_offsets[i] = offset;
            vm->string_lengths[i] = (uint32_t)len;
            vm->string_count = (size_t)i + 1;
        }
    }

    int again = JS_ToBool(ctx, argv[2]);
    if (again < 0) {
        return JS_EXCEPTION;
    }
    vm->op_count = (size_t)len_d;
    vm->again = again != 0;
    vm->flushed = true;
    return JS_UNDEFINED;
}

static JSValue peek_js_log(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv) {
    (void)this_val;
    PeekVM *vm = (PeekVM *)JS_GetContextOpaque(ctx);
    if (vm->log == NULL) {
        return JS_UNDEFINED;
    }
    size_t len = 0;
    const char *str = JS_ToCStringLen(ctx, &len, argc > 0 ? argv[0] : JS_UNDEFINED);
    if (str == NULL) {
        return JS_EXCEPTION;
    }
    vm->log(vm->log_opaque, str, len);
    JS_FreeCString(ctx, str);
    return JS_UNDEFINED;
}

static JSValue peek_js_measure(JSContext *ctx, JSValueConst this_val, int argc, JSValueConst *argv) {
    (void)this_val;
    PeekVM *vm = (PeekVM *)JS_GetContextOpaque(ctx);
    double metrics[PEEK_TEXT_METRICS_COUNT];
    memset(metrics, 0, sizeof metrics);

    if (vm->measure != NULL) {
        size_t font_len = 0;
        const char *font = JS_ToCStringLen(ctx, &font_len, argc > 0 ? argv[0] : JS_UNDEFINED);
        if (font == NULL) {
            return JS_EXCEPTION;
        }
        size_t text_len = 0;
        const char *text = JS_ToCStringLen(ctx, &text_len, argc > 1 ? argv[1] : JS_UNDEFINED);
        if (text == NULL) {
            JS_FreeCString(ctx, font);
            return JS_EXCEPTION;
        }
        vm->measure(vm->measure_opaque, font, font_len, text, text_len, metrics);
        JS_FreeCString(ctx, text);
        JS_FreeCString(ctx, font);
    }

    JSValue result = JS_NewArray(ctx);
    if (JS_IsException(result)) {
        return result;
    }
    for (uint32_t i = 0; i < PEEK_TEXT_METRICS_COUNT; i++) {
        double value = isfinite(metrics[i]) ? metrics[i] : 0;
        if (JS_SetPropertyUint32(ctx, result, i, JS_NewFloat64(ctx, value)) < 0) {
            JS_FreeValue(ctx, result);
            return JS_EXCEPTION;
        }
    }
    return result;
}

static bool peek_install_globals(PeekVM *vm) {
    JSContext *ctx = vm->ctx;
    JSValue global = JS_GetGlobalObject(ctx);
    bool ok = false;

    JSValue buffer = JS_NewArrayBuffer(ctx, (uint8_t *)vm->ops, vm->op_capacity * sizeof(double), 0, NULL,
                                       NULL, false);
    if (JS_IsException(buffer)) {
        goto done;
    }
    JSValue view = JS_NewTypedArray(ctx, 1, &buffer, JS_TYPED_ARRAY_FLOAT64);
    JS_FreeValue(ctx, buffer);
    if (JS_IsException(view)) {
        goto done;
    }
    if (JS_SetPropertyStr(ctx, global, "__peek_ops", view) < 0) {
        goto done;
    }
    JSValue flush = JS_NewCFunction(ctx, peek_js_flush, "__peek_flush", 3);
    if (JS_IsException(flush) || JS_SetPropertyStr(ctx, global, "__peek_flush", flush) < 0) {
        goto done;
    }
    JSValue log = JS_NewCFunction(ctx, peek_js_log, "__peek_log", 1);
    if (JS_IsException(log) || JS_SetPropertyStr(ctx, global, "__peek_log", log) < 0) {
        goto done;
    }
    JSValue measure = JS_NewCFunction(ctx, peek_js_measure, "__peek_measure", 2);
    if (JS_IsException(measure) || JS_SetPropertyStr(ctx, global, "__peek_measure", measure) < 0) {
        goto done;
    }
    ok = true;

done:
    if (!ok) {
        JSValue exc = JS_GetException(ctx);
        JS_FreeValue(ctx, exc);
    }
    JS_FreeValue(ctx, global);
    return ok;
}

/* ---- lifecycle ---------------------------------------------------------- */

PeekVM *peek_vm_new(const PeekVMConfig *config) {
    PeekVMConfig cfg;
    memset(&cfg, 0, sizeof cfg);
    if (config != NULL) {
        cfg = *config;
    }

    PeekVM *vm = (PeekVM *)calloc(1, sizeof(PeekVM));
    if (vm == NULL) {
        return NULL;
    }
    vm->frame_fn = JS_UNDEFINED;
    vm->event_fn = JS_UNDEFINED;
    atomic_init(&vm->abort_requested, false);
    vm->log = cfg.log;
    vm->log_opaque = cfg.log_opaque;
    vm->measure = cfg.measure;
    vm->measure_opaque = cfg.measure_opaque;

    vm->op_capacity = cfg.op_capacity ? cfg.op_capacity : PEEK_DEFAULT_OP_CAPACITY;
    if (vm->op_capacity > SIZE_MAX / sizeof(double) || vm->op_capacity > UINT32_MAX) {
        free(vm);
        return NULL;
    }
    vm->ops = (double *)calloc(vm->op_capacity, sizeof(double));
    if (vm->ops == NULL) {
        free(vm);
        return NULL;
    }

    vm->rt = JS_NewRuntime();
    if (vm->rt == NULL) {
        peek_vm_free(vm);
        return NULL;
    }
    JS_SetRuntimeOpaque(vm->rt, vm);
    JS_SetMemoryLimit(vm->rt, cfg.memory_limit ? cfg.memory_limit : PEEK_DEFAULT_MEMORY_LIMIT);
    JS_SetMaxStackSize(vm->rt, cfg.max_stack_size ? cfg.max_stack_size : PEEK_DEFAULT_MAX_STACK_SIZE);
    JS_SetInterruptHandler(vm->rt, peek_interrupt_handler, vm);

    vm->ctx = JS_NewContextRaw(vm->rt);
    if (vm->ctx == NULL) {
        peek_vm_free(vm);
        return NULL;
    }
    JS_SetContextOpaque(vm->ctx, vm);

    if (JS_AddIntrinsicBaseObjects(vm->ctx) != 0 || JS_AddIntrinsicEval(vm->ctx) != 0 ||
        JS_AddIntrinsicRegExp(vm->ctx) != 0 || JS_AddIntrinsicJSON(vm->ctx) != 0 ||
        JS_AddIntrinsicMapSet(vm->ctx) != 0 || JS_AddIntrinsicTypedArrays(vm->ctx) != 0 ||
        JS_AddIntrinsicPromise(vm->ctx) != 0) {
        peek_vm_free(vm);
        return NULL;
    }
    JS_AddIntrinsicRegExpCompiler(vm->ctx);

    if (!peek_install_globals(vm)) {
        peek_vm_free(vm);
        return NULL;
    }
    return vm;
}

void peek_vm_free(PeekVM *vm) {
    if (vm == NULL) {
        return;
    }
    if (vm->ctx != NULL) {
        JS_FreeValue(vm->ctx, vm->frame_fn);
        JS_FreeValue(vm->ctx, vm->event_fn);
        JS_FreeContext(vm->ctx);
    }
    if (vm->rt != NULL) {
        JS_FreeRuntime(vm->rt);
    }
    free(vm->ops);
    free(vm->string_bytes);
    free(vm->string_offsets);
    free(vm->string_lengths);
    free(vm->scratch);
    free(vm->error_message);
    free(vm->error_stack);
    free(vm);
}

/* ---- calls -------------------------------------------------------------- */

PeekStatus peek_vm_eval(PeekVM *vm, const char *source, size_t len, const char *filename, bool strict,
                        uint64_t deadline_ns) {
    if (vm == NULL || (source == NULL && len > 0)) {
        return PEEK_STATUS_INVALID_ARGUMENT;
    }
    const char *terminated = peek_scratch_copy(vm, source, len);
    if (terminated == NULL) {
        peek_set_error_literal(vm, "InternalError: out of memory copying the script");
        return PEEK_STATUS_OUT_OF_MEMORY;
    }

    peek_begin_call(vm, deadline_ns);
    int flags = JS_EVAL_TYPE_GLOBAL | (strict ? JS_EVAL_FLAG_STRICT : 0);
    JSValue result = JS_Eval(vm->ctx, terminated, len, filename ? filename : "<script>", flags);
    PeekStatus status = PEEK_STATUS_OK;
    if (JS_IsException(result)) {
        status = peek_capture_exception(vm);
    } else {
        JS_FreeValue(vm->ctx, result);
        status = peek_run_pending_jobs(vm);
    }
    peek_end_call(vm);
    return status;
}

PeekStatus peek_vm_bind_entry_points(PeekVM *vm, bool hide_globals) {
    if (vm == NULL) {
        return PEEK_STATUS_INVALID_ARGUMENT;
    }
    JSContext *ctx = vm->ctx;
    JS_UpdateStackTop(vm->rt);
    JSValue global = JS_GetGlobalObject(ctx);
    JSValue frame = JS_GetPropertyStr(ctx, global, "__peek_frame");
    JSValue event = JS_GetPropertyStr(ctx, global, "__peek_event");
    PeekStatus status = PEEK_STATUS_OK;

    if (JS_IsException(frame) || JS_IsException(event)) {
        status = peek_capture_exception(vm);
        goto done;
    }
    if (!JS_IsFunction(ctx, frame)) {
        peek_set_error_literal(vm, "TypeError: the prelude did not define a __peek_frame(input) function");
        status = PEEK_STATUS_MISSING_ENTRY;
        goto done;
    }

    JS_FreeValue(ctx, vm->frame_fn);
    JS_FreeValue(ctx, vm->event_fn);
    vm->frame_fn = JS_DupValue(ctx, frame);
    vm->event_fn = JS_IsFunction(ctx, event) ? JS_DupValue(ctx, event) : JS_UNDEFINED;
    vm->bound = true;

    if (hide_globals) {
        static const char *const names[] = {"__peek_frame", "__peek_event", "__peek_ops", "__peek_flush",
                                            "__peek_log", "__peek_measure"};
        for (size_t i = 0; i < sizeof names / sizeof names[0]; i++) {
            JSAtom atom = JS_NewAtom(ctx, names[i]);
            if (atom == JS_ATOM_NULL) {
                status = peek_capture_exception(vm);
                goto done;
            }
            int deleted = JS_DeleteProperty(ctx, global, atom, 0);
            JS_FreeAtom(ctx, atom);
            if (deleted < 0) {
                status = peek_capture_exception(vm);
                goto done;
            }
        }
    }

done:
    JS_FreeValue(ctx, frame);
    JS_FreeValue(ctx, event);
    JS_FreeValue(ctx, global);
    return status;
}

static void peek_fill_output(const PeekVM *vm, PeekFrameOutput *out) {
    out->flushed = vm->flushed;
    out->again = vm->flushed && vm->again;
    out->ops = vm->ops;
    out->op_count = vm->flushed ? vm->op_count : 0;
    out->string_bytes = vm->string_bytes ? vm->string_bytes : peek_empty_string;
    out->string_offsets = vm->string_offsets;
    out->string_lengths = vm->string_lengths;
    out->string_count = vm->flushed ? vm->string_count : 0;
}

PeekStatus peek_vm_frame(PeekVM *vm, const char *input_json, size_t len, uint64_t deadline_ns,
                         PeekFrameOutput *out) {
    if (out != NULL) {
        memset(out, 0, sizeof *out);
        out->string_bytes = peek_empty_string;
    }
    if (vm == NULL || out == NULL || (input_json == NULL && len > 0)) {
        return PEEK_STATUS_INVALID_ARGUMENT;
    }
    if (!vm->bound) {
        peek_set_error_literal(vm, "TypeError: peek_vm_bind_entry_points has not bound __peek_frame");
        return PEEK_STATUS_MISSING_ENTRY;
    }
    JSContext *ctx = vm->ctx;
    vm->flushed = false;
    vm->op_count = 0;
    vm->string_count = 0;
    vm->string_bytes_len = 0;

    const char *terminated = peek_scratch_copy(vm, input_json, len);
    if (terminated == NULL) {
        peek_set_error_literal(vm, "InternalError: out of memory copying the frame input");
        return PEEK_STATUS_OUT_OF_MEMORY;
    }

    peek_begin_call(vm, deadline_ns);
    PeekStatus status = PEEK_STATUS_OK;
    JSValue input = len > 0 ? JS_ParseJSON(ctx, terminated, len, "<input>") : JS_UNDEFINED;
    if (JS_IsException(input)) {
        status = peek_capture_exception(vm);
        if (status == PEEK_STATUS_THREW) {
            status = PEEK_STATUS_INVALID_ARGUMENT;
        }
        peek_end_call(vm);
        return status;
    }

    JSValue result = JS_Call(ctx, vm->frame_fn, JS_UNDEFINED, 1, (JSValueConst *)&input);
    JS_FreeValue(ctx, input);
    if (JS_IsException(result)) {
        status = peek_capture_exception(vm);
    } else {
        JS_FreeValue(ctx, result);
        status = peek_run_pending_jobs(vm);
    }
    peek_end_call(vm);

    if (status == PEEK_STATUS_OK) {
        peek_fill_output(vm, out);
    } else {
        vm->flushed = false;
    }
    return status;
}

PeekStatus peek_vm_event(PeekVM *vm, const char *name, const char *payload_json, size_t len,
                         uint64_t deadline_ns) {
    if (vm == NULL || name == NULL || (payload_json == NULL && len > 0)) {
        return PEEK_STATUS_INVALID_ARGUMENT;
    }
    if (!vm->bound) {
        peek_set_error_literal(vm, "TypeError: peek_vm_bind_entry_points has not bound __peek_frame");
        return PEEK_STATUS_MISSING_ENTRY;
    }
    if (JS_IsUndefined(vm->event_fn)) {
        return PEEK_STATUS_OK;
    }
    JSContext *ctx = vm->ctx;

    peek_begin_call(vm, deadline_ns);
    PeekStatus status = PEEK_STATUS_OK;
    JSValue payload = JS_UNDEFINED;
    if (payload_json != NULL && len > 0) {
        const char *terminated = peek_scratch_copy(vm, payload_json, len);
        if (terminated == NULL) {
            peek_end_call(vm);
            peek_set_error_literal(vm, "InternalError: out of memory copying the event payload");
            return PEEK_STATUS_OUT_OF_MEMORY;
        }
        payload = JS_ParseJSON(ctx, terminated, len, "<event>");
        if (JS_IsException(payload)) {
            status = peek_capture_exception(vm);
            if (status == PEEK_STATUS_THREW) {
                status = PEEK_STATUS_INVALID_ARGUMENT;
            }
            peek_end_call(vm);
            return status;
        }
    }
    JSValue name_value = JS_NewString(ctx, name);
    if (JS_IsException(name_value)) {
        JS_FreeValue(ctx, payload);
        status = peek_capture_exception(vm);
        peek_end_call(vm);
        return status;
    }

    JSValueConst args[2] = {name_value, payload};
    JSValue result = JS_Call(ctx, vm->event_fn, JS_UNDEFINED, 2, args);
    JS_FreeValue(ctx, name_value);
    JS_FreeValue(ctx, payload);
    if (JS_IsException(result)) {
        status = peek_capture_exception(vm);
    } else {
        JS_FreeValue(ctx, result);
        status = peek_run_pending_jobs(vm);
    }
    peek_end_call(vm);
    return status;
}

/* ---- accessors ---------------------------------------------------------- */

const char *peek_vm_error_message(const PeekVM *vm, size_t *len) {
    if (vm == NULL || vm->error_message == NULL) {
        if (len != NULL) {
            *len = 0;
        }
        return peek_empty_string;
    }
    if (len != NULL) {
        *len = vm->error_message_len;
    }
    return vm->error_message;
}

const char *peek_vm_error_stack(const PeekVM *vm, size_t *len) {
    if (vm == NULL || vm->error_stack == NULL) {
        if (len != NULL) {
            *len = 0;
        }
        return peek_empty_string;
    }
    if (len != NULL) {
        *len = vm->error_stack_len;
    }
    return vm->error_stack;
}

void peek_vm_request_abort(PeekVM *vm) {
    if (vm != NULL) {
        atomic_store_explicit(&vm->abort_requested, true, memory_order_relaxed);
    }
}

void peek_vm_clear_abort(PeekVM *vm) {
    if (vm != NULL) {
        atomic_store_explicit(&vm->abort_requested, false, memory_order_relaxed);
    }
}

size_t peek_vm_memory_used(PeekVM *vm) {
    if (vm == NULL) {
        return 0;
    }
    JSMemoryUsage usage;
    JS_ComputeMemoryUsage(vm->rt, &usage);
    return usage.memory_used_size > 0 ? (size_t)usage.memory_used_size : 0;
}

void peek_vm_run_gc(PeekVM *vm) {
    if (vm != NULL) {
        JS_UpdateStackTop(vm->rt);
        JS_RunGC(vm->rt);
    }
}

size_t peek_vm_op_capacity(const PeekVM *vm) { return vm ? vm->op_capacity : 0; }
