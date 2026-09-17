import type { SettingsPatch, SettingsPayload, SettingsState } from "./settings.ts";

/** Preserve user intent order for overlapping fields without blocking unrelated saves. */
export function createSettingsPatchWriter(save: (patch: SettingsPatch) => Promise<SettingsPayload>) {
  const pending = new Map<keyof SettingsState, Promise<SettingsPayload>>();
  return (patch: SettingsPatch): Promise<SettingsPayload> => {
    const fields = Object.keys(patch) as (keyof SettingsState)[];
    const preceding = fields.map((field) => pending.get(field)).filter((entry) => entry !== undefined);
    const result = Promise.allSettled(preceding).then(() => save(patch));
    for (const field of fields) pending.set(field, result);
    const release = () => {
      for (const field of fields) if (pending.get(field) === result) pending.delete(field);
    };
    void result.then(release, release);
    return result;
  };
}
