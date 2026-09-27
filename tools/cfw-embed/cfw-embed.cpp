// cfw-embed — Card Studio's Needle embedding helper.
//
// Built with the llvm-mingw toolchain (clang++, static libc++), NOT by the
// Rust workspace: the shipped libneedle.a is clang/libc++ over mingw/UCRT,
// which MSVC's link.exe rejects (LNK1143) and which Rust's windows-gnu
// target cannot link cleanly (msvcrt crt2.o vs UCRT libc++). See
// docs/superpowers/specs/2026-09-26-needle-integration-design.md, PR 2a
// amendment. Build with tools/cfw-embed/build.ps1.
//
// Protocol (stdio, line-based, one response line-set per request):
//   PING                              -> "PONG <dim>"
//   EMBED <byte_len> \n <len bytes> \n -> "OK <wrote>" \n <wrote floats CSV>
//                                     or "ERR <message>" (process lives on)
// Any other op -> "ERR unknown op". EOF on stdin exits 0. Length-prefixed
// payloads mean no escaping rules; stems with embedded newlines are
// rejected client-side and tolerated here anyway via the byte count.
// embed_many is a client-side loop of EMBED over this one warm process.
//
// Telemetry is disabled before any engine call. The engine never touches
// the network; this process must never gain a reason to.

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "needle.h"

static float *g_vec = NULL;
static int g_dim = 0;

static void emit_err(const char *msg) { printf("ERR %s\n", msg); fflush(stdout); }

static int read_exact_line(char *buf, size_t cap) {
    // Reads one line (consuming its '\n'). Returns length, or -1 on EOF/err.
    size_t i = 0;
    int c;
    while ((c = getchar()) != EOF) {
        if (c == '\n') { buf[i] = '\0'; return (int)i; }
        if (i + 1 >= cap) return -1;
        buf[i++] = (char)c;
    }
    return -1;
}

int main(int argc, char **argv) {
    // Telemetry off before the engine loads.
    _putenv("NEEDLE_TELEMETRY=0");
    _putenv("DO_NOT_TRACK=1");

    const char *weights_path = NULL;
    for (int i = 1; i < argc - 1; i++) {
        if (strcmp(argv[i], "--weights") == 0) weights_path = argv[i + 1];
    }
    if (!weights_path) { fprintf(stderr, "usage: cfw-embed --weights <needle3.cact>\n"); return 2; }

    FILE *f = fopen(weights_path, "rb");
    if (!f) { fprintf(stderr, "cfw-embed: cannot open weights: %s\n", weights_path); return 1; }
    fseek(f, 0, SEEK_END);
    long n = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (n <= 0) { fprintf(stderr, "cfw-embed: empty weights file\n"); return 1; }
    unsigned char *bytes = (unsigned char *)malloc((size_t)n);
    if (fread(bytes, 1, (size_t)n, f) != (size_t)n) { fprintf(stderr, "cfw-embed: short weights read\n"); return 1; }
    fclose(f);

    if (needle_load(bytes, (unsigned long long)n) < 0) {
        fprintf(stderr, "cfw-embed: needle_load failed: %s\n", needle_last_error());
        return 1;
    }
    if (needle_init(NULL, NULL, NULL) < 0) {
        fprintf(stderr, "cfw-embed: needle_init failed: %s\n", needle_last_error());
        return 1;
    }
    g_dim = needle_embed("warmup", NULL, 0);
    if (g_dim <= 0) { fprintf(stderr, "cfw-embed: no embedding dimension\n"); return 1; }
    g_vec = (float *)malloc(sizeof(float) * (size_t)g_dim);
    if (!g_vec) { fprintf(stderr, "cfw-embed: out of memory\n"); return 1; }

    char line[64];
    char *stem = NULL;
    size_t stem_cap = 0;
    for (;;) {
        int len = read_exact_line(line, sizeof(line));
        if (len < 0) break; // EOF: exit cleanly
        if (strcmp(line, "PING") == 0) {
            printf("PONG %d\n", g_dim);
            fflush(stdout);
            continue;
        }
        if (strncmp(line, "EMBED ", 6) == 0) {
            long want = strtol(line + 6, NULL, 10);
            if (want <= 0 || want > 65536) { emit_err("bad stem length"); continue; }
            if ((size_t)want + 1 > stem_cap) {
                free(stem);
                stem_cap = (size_t)want + 1;
                stem = (char *)malloc(stem_cap);
                if (!stem) { emit_err("out of memory"); break; }
            }
            size_t got = 0;
            while (got < (size_t)want) {
                int c = getchar();
                if (c == EOF) { got = (size_t)-1; break; }
                stem[got++] = (char)c;
            }
            if (got == (size_t)-1) break;             // EOF mid-payload
            if (getchar() != '\n') { emit_err("unterminated stem"); continue; }
            stem[want] = '\0';
            int wrote = needle_embed(stem, g_vec, g_dim);
            if (wrote < 0) { emit_err(needle_last_error()); continue; }
            printf("OK %d\n", wrote);
            for (int i = 0; i < wrote; i++) {
                printf(i ? " %.9g" : "%.9g", (double)g_vec[i]);
            }
            printf("\n");
            fflush(stdout);
            continue;
        }
        emit_err("unknown op");
    }
    free(stem);
    return 0;
}
