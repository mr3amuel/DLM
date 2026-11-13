#include <stddef.h>
#include <stdint.h>

typedef struct DlmExperiment DlmExperiment;

// Inputs are borrowed UTF-8 bytes for the duration of the call.
// Every non-null output is an owned JSON C string. Free it with dlm_free_json.
// The caller serializes all operations for each handle, off the UI thread.
// Open fills out only on success. Close consumes a non-null handle even on error.
uint32_t dlm_abi_version(void);
char *dlm_open(const uint8_t *json, size_t length, DlmExperiment **out);
char *dlm_call(DlmExperiment *engine, const uint8_t *json, size_t length);
char *dlm_close(DlmExperiment *engine);
void dlm_free_json(char *result);
