import { useEffect, useRef, useState } from "react";
import { Dialog } from "radix-ui";

import { AgentOptionLabel } from "../../components/shared/AgentOptionLabel.tsx";
import { DialogActionBar } from "../../components/shared/DialogActionBar.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogAdvanceButton } from "../../components/shared/DialogAdvanceButton.tsx";
import { DialogTextField } from "../../components/shared/DialogTextField.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { skillSourceErrorMessage } from "../../lib/add-skill-dialog.ts";
import { createLocalSkill } from "../../lib/runtime-gateway.ts";
import type { RawSkillRecord } from "../../lib/skills.ts";

type CreateLocalSkillFormProps = {
  target: string;
  targetOptions: Array<{ id: string; displayName: string }>;
  onTargetChange: (target: string) => void;
  onBusyChange: (busy: boolean) => void;
  onBeginMutation: () => () => void;
  onCreated: (skills: RawSkillRecord[]) => void;
  onBack: () => void;
  onClose: () => void;
};

export function CreateLocalSkillForm({
  target, targetOptions, onTargetChange, onBusyChange, onBeginMutation, onCreated, onBack, onClose,
}: CreateLocalSkillFormProps) {
  const [name, setName] = useState("");
  const [nameEdited, setNameEdited] = useState(false);
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const formRef = useRef<HTMLFormElement>(null);
  const normalizedName = name.trim();
  const validName = name.length <= 64 && /^[a-z0-9]+(-[a-z0-9]+)*$/.test(name);
  const nameError = !nameEdited || validName ? ""
    : !name ? "Enter a name."
      : name.length > 64 ? "Use up to 64 characters."
        : "Use lowercase letters, numbers, and hyphens.";
  const canCreate = validName && Boolean(description.trim()) && Boolean(target) && !busy;

  useEffect(() => { setError(""); }, [name, description, target]);

  const create = async () => {
    if (!canCreate) return;
    setBusy(true);
    onBusyChange(true);
    setError("");
    const finishMutation = onBeginMutation();
    try {
      const skills = await createLocalSkill({ name: normalizedName, description: description.trim(), target });
      onCreated(skills);
      onClose();
    } catch (creationError) {
      setError(skillSourceErrorMessage(creationError));
    } finally {
      finishMutation();
      setBusy(false);
      onBusyChange(false);
    }
  };

  return (
    <>
      <div className="addSkillBody">
        <div className="addSkillMain">
          <div className="addSkillHeader">
            <Dialog.Title className="confirmDialogTitle">Create local skill</Dialog.Title>
          </div>
          <Dialog.Description id="add-skill-dialog-description" className="dialogVisuallyHidden">
            Create a local skill and open it in the editor.
          </Dialog.Description>
          <form
            ref={formRef}
            className="addSkillGrid createLocalSkillBody"
            noValidate
            onSubmit={(event) => { event.preventDefault(); setNameEdited(true); void create(); }}
          >
            <DialogTextField
              label="Name" value={name} onChange={(value) => { setNameEdited(true); setName(value); }} placeholder="my-skill"
              disabled={busy} autoFocus required maxLength={64} pattern="[a-z0-9]+(-[a-z0-9]+)*"
              error={nameError}
            />
            <label className="dialogField">
              <span>Description</span>
              <textarea
                className="dialogTextArea" value={description} disabled={busy} required
                placeholder="When should the agent use this skill?"
                onChange={(event) => setDescription(event.target.value)}
              />
            </label>
            <div className="dialogField">
              <span>Create in</span>
              <SelectControl
                value={target} onValueChange={onTargetChange} label="Local skill target" disabled={busy}
                contentClassName="dialogSelectContent"
                options={targetOptions.map((option) => ({ value: option.id, label: option.displayName }))}
                renderValue={(option) => option ? <AgentOptionLabel agent={option.value} label={option.label} /> : null}
                renderOption={(option) => <AgentOptionLabel agent={option.value} label={option.label} />}
              />
            </div>
            {error ? <Toast tone="error" message={error} /> : null}
          </form>
        </div>
      </div>
      <DialogActionBar
        onCancel={onClose} cancelDisabled={busy}
        leading={<DialogActionButton variant="secondary" disabled={busy} onClick={onBack}>Back</DialogActionButton>}
      >
        <DialogAdvanceButton
          label="Create" busyLabel="Creating local skill" busy={busy} disabled={!canCreate}
          onClick={() => formRef.current?.requestSubmit()}
        />
      </DialogActionBar>
    </>
  );
}
