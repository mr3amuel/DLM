#include "../include/dlm_experiment.h"
// Test executable only. No Rust library or production data is used.
enum { TEST_SNAPSHOT, TEST_SCHEDULES, TEST_ORGANIZATION, TEST_ADD, TEST_CLOSE, TEST_OPEN, TEST_INSPECT, TEST_SOURCE, TEST_PARTIAL, TEST_SAVE, TEST_DISMISS, TEST_RESUME, TEST_BROWSER_STATE, TEST_BROWSER_TAKE, TEST_BROWSER_ACK, TEST_BROWSER_ENABLE, TEST_BROWSER_RESET, TEST_BROWSER_CODE, TEST_OPERATIONS };
void lifecycle_reset(void);
void lifecycle_fail(int operation, int count);
int lifecycle_count(int operation);
int lifecycle_live_handles(void);

void lifecycle_schedule_json(const char *json);

void lifecycle_browser_json(const char *json);
