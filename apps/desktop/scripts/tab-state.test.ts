import assert from "node:assert/strict";
import test, { mock } from "node:test";

if (typeof mock.module !== "function") {
  test("tab state behavior tests require Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  const hookRefs: Array<{ current: unknown }> = [];
  let hookIndex = 0;

  mock.module("react", {
    namedExports: {
      useCallback: (callback: unknown) => callback,
      useRef: (initialValue: unknown) => {
        const ref = hookRefs[hookIndex] ?? { current: initialValue };
        hookRefs[hookIndex] = ref;
        hookIndex += 1;
        return ref;
      },
      useSyncExternalStore: (_subscribe: unknown, getSnapshot: () => unknown) => getSnapshot(),
    },
  });

  const { getTabScrollPosition, setTabScrollPosition, useTabState } = await import("../src/lib/tab-state.ts");

  function render<T>(key: string, initialValue: T | (() => T)) {
    hookIndex = 0;
    return useTabState(key, initialValue);
  }

  function remount<T>(key: string, initialValue: T | (() => T)) {
    hookRefs.length = 0;
    return render(key, initialValue);
  }

  test("restores state after an unmount and evaluates lazy initial values once", () => {
    let initializerCalls = 0;
    const initial = render("skills", () => {
      initializerCalls += 1;
      return "";
    });

    initial[1]("codex");
    const restored = remount("skills", () => {
      initializerCalls += 1;
      return "fallback";
    });

    assert.equal(restored[0], "codex");
    assert.equal(initializerCalls, 1);
  });

  test("isolates values by string key and supports functional updaters", () => {
    const first = render("sessions", 1);
    const second = render("rules", 10);

    first[1]((value) => value + 1);
    second[1](20);

    assert.equal(remount("sessions", 0)[0], 2);
    assert.equal(remount("rules", 0)[0], 20);
  });

  test("stores scroll positions by tab key", () => {
    setTabScrollPosition("skills.list", { top: 240, left: 18 });
    assert.deepEqual(getTabScrollPosition("skills.list"), { top: 240, left: 18 });
    assert.equal(getTabScrollPosition("sessions.list"), undefined);
  });
}
