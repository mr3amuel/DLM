#include "lifecycle_ffi.h"
#include <stdlib.h>
#include <string.h>
#include <assert.h>

struct DlmExperiment { int live; };
static int counts[TEST_OPERATIONS], failures[TEST_OPERATIONS], live;
static char *schedule_json, *browser_json;
static int browser_enabled;
void lifecycle_browser_json(const char *json) {
    free(browser_json);
    browser_json = json ? strdup(json) : NULL;
}
void lifecycle_schedule_json(const char *json) {
    free(schedule_json);
    schedule_json = json ? strdup(json) : NULL;
}
void lifecycle_reset(void) {
    assert(live == 0);
    lifecycle_schedule_json(NULL);
    lifecycle_browser_json(NULL);
    browser_enabled = 0;
    memset(counts, 0, sizeof counts);
    memset(failures, 0, sizeof failures);
}
void lifecycle_fail(int op, int count) { failures[op] = count; }
int lifecycle_count(int op) { return counts[op]; }
int lifecycle_live_handles(void) { return live; }
static char *reply(int op, const char *success) {
    counts[op]++;
    if (failures[op] > 0) {
        failures[op]--;
        return strdup("{\"error\":{\"code\":\"fixture\",\"message\":\"Injected lifecycle failure\"}}");
    }
    return strdup(success);
}
uint32_t dlm_abi_version(void) { return 1; }
char *dlm_open(const uint8_t *json, size_t length, DlmExperiment **out) {
    (void)json; (void)length;
    *out = calloc(1, sizeof **out);
    (*out)->live = 1;
    live++;
    return reply(TEST_OPEN, "{\"data\":\"opened\"}");
}
char *dlm_call(DlmExperiment *engine, const uint8_t *json, size_t length) {
    assert(engine && engine->live);
    char *input = strndup((const char *)json, length);
    int op = strstr(input, "\"browser_state\"") ? TEST_BROWSER_STATE :
        strstr(input, "\"browser_take\"") ? TEST_BROWSER_TAKE :
        strstr(input, "\"browser_acknowledge\"") ? TEST_BROWSER_ACK :
        strstr(input, "\"browser_enable\"") ? TEST_BROWSER_ENABLE :
        strstr(input, "\"browser_reset\"") ? TEST_BROWSER_RESET :
        strstr(input, "\"browser_pairing_code\"") ? TEST_BROWSER_CODE :
        strstr(input, "\"snapshot\"") ? TEST_SNAPSHOT :
        strstr(input, "\"schedules\"") ? TEST_SCHEDULES :
        strstr(input, "\"organization\"") ? TEST_ORGANIZATION :
        strstr(input, "\"inspect_draft\"") ? TEST_INSPECT :
        strstr(input, "\"inspect_source\"") ? TEST_SOURCE :
        strstr(input, "\"partial_info\"") ? TEST_PARTIAL :
        strstr(input, "\"save_schedule\"") ? TEST_SAVE :
        strstr(input, "\"dismiss_missed\"") ? TEST_DISMISS :
        strstr(input, "\"resume\"") ? TEST_RESUME : TEST_ADD;
    if (failures[op] == 0) {
        if (op == TEST_BROWSER_ENABLE) { browser_enabled = strstr(input, "true") != NULL; }
        if (op == TEST_BROWSER_ACK || op == TEST_BROWSER_RESET || (op == TEST_BROWSER_ENABLE && !browser_enabled)) {
            lifecycle_browser_json(NULL);
        }
    }
    free(input);
    if (op == TEST_BROWSER_TAKE) return reply(op, browser_json ? browser_json : "{\"data\":null}");
    if (op == TEST_BROWSER_STATE) return reply(op, browser_enabled
        ? "{\"data\":{\"enabled\":true,\"directory\":\"/fixture/browser-integration\"}}"
        : "{\"data\":{\"enabled\":false,\"directory\":\"/fixture/browser-integration\"}}");
    if (op == TEST_BROWSER_CODE) return reply(op, "{\"data\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}");
    if (op == TEST_BROWSER_ENABLE || op == TEST_BROWSER_RESET || op == TEST_BROWSER_ACK) return reply(op, "{\"data\":true}");
    const char *data = op == TEST_SAVE ? "{\"data\":\"schedule-id\"}" :
        (op == TEST_DISMISS || op == TEST_RESUME) ? "{\"data\":null}" :
        op == TEST_PARTIAL ? "{\"data\":{\"exists\":true,\"file_length\":123,\"retained_verified_bytes\":120}}" :
        op == TEST_INSPECT ? "{\"data\":{\"directory\":\"/resolved/Videos\",\"category_id\":\"videos\",\"duplicates\":[{\"id\":\"existing-id\",\"filename\":\"existing.bin\",\"destination\":\"/resolved/Videos/existing.bin\",\"reasons\":[\"url\"]},{\"id\":\"\",\"filename\":\"file.bin\",\"destination\":\"/resolved/Videos/file.bin\",\"reasons\":[\"destination\"]}]}}" :
        op == TEST_SOURCE ? "{\"data\":{\"url\":\"https://example.invalid/file\"}}" :
        op == TEST_SNAPSHOT ? "{\"data\":[{\"id\":\"existing-id\",\"filename\":\"existing.bin\",\"destination\":\"/unused-fixture/existing.bin\",\"status\":\"Paused\",\"downloaded\":123,\"durable_bytes\":120,\"speed\":0,\"url\":\"https://example.invalid/existing\",\"resume_capability\":\"supported\",\"workers\":[],\"priority\":\"Normal\",\"connections\":4,\"optimize\":true,\"safe_pause\":true,\"active_connections\":0,\"target_connections\":1,\"segmented\":false}]}" :
        op == TEST_SCHEDULES ? "{\"data\":{\"rules\":[],\"manual_override\":false}}" :
        op == TEST_ORGANIZATION ? "{\"data\":{\"enabled\":false,\"categories\":[]}}" :
        "{\"data\":\"created-id\"}";
    return reply(op, op == TEST_SCHEDULES && schedule_json ? schedule_json : data);
}
char *dlm_close(DlmExperiment *engine) {
    if (engine) {
        assert(engine->live);
        engine->live = 0;
        free(engine); // Match Rust: consumed even when returning an error.
        live--;
    }
    return reply(TEST_CLOSE, "{\"data\":null}");
}
void dlm_free_json(char *result) { free(result); }
