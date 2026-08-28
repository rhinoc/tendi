import assert from "node:assert/strict";
import test from "node:test";

import { backupDialogLeadingAction } from "../src/lib/backup-dialog.ts";

test("details page uses Back as the bottom-left action", () => {
  assert.equal(backupDialogLeadingAction(true, true), "back");
});

test("details page keeps Back when backup is not configured", () => {
  assert.equal(backupDialogLeadingAction(true, false), "back");
});

test("backup details home keeps Disconnect for configured backup", () => {
  assert.equal(backupDialogLeadingAction(false, true), "disconnect");
});

test("unconfigured backup details home has no leading action", () => {
  assert.equal(backupDialogLeadingAction(false, false), null);
});
