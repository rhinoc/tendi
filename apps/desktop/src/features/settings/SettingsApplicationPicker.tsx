import { AlertCircle, Check, ChevronDown, Play } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { useRef, useState } from "react";

import { IconButton } from "../../components/shared/IconButton.tsx";
import { LoadingIcon } from "../../components/shared/LoadingIcon.tsx";
import { StatefulButton } from "../../components/shared/StatefulButton.tsx";

export type SettingsApplicationOption = {
  value: string;
  label: string;
  available?: boolean;
};

type TestState = "idle" | "loading" | "success" | "error";

export type SettingsApplicationPickerProps = {
  id: string;
  ariaLabel: string;
  menuAriaLabel: string;
  placeholder: string;
  value: string;
  savedValue: string;
  options: SettingsApplicationOption[];
  error?: string;
  labels: {
    opening: string;
    opened: string;
    failed: string;
    test: string;
  };
  onChange: (value: string) => void;
  onSave: (value: string) => void | Promise<void>;
  onCancel: () => void;
  onTest: (value: string) => Promise<boolean>;
};

export function SettingsApplicationPicker({
  id,
  ariaLabel,
  menuAriaLabel,
  placeholder,
  value,
  savedValue,
  options,
  error,
  labels,
  onChange,
  onSave,
  onCancel,
  onTest,
}: SettingsApplicationPickerProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const [testState, setTestState] = useState<TestState>("idle");
  const testRequestRef = useRef(0);
  const selectedOption = options.find((option) => option.value === value);
  const displayValue = selectedOption?.label ?? value;

  const resetTestState = () => {
    testRequestRef.current += 1;
    setTestState("idle");
  };

  const chooseOption = (nextValue: string) => {
    onChange(nextValue);
    setMenuOpen(false);
    resetTestState();
    void onSave(nextValue);
  };

  const testApplication = async () => {
    const application = value.trim();
    if (!application) return;
    const requestId = testRequestRef.current + 1;
    testRequestRef.current = requestId;
    setTestState("loading");
    const succeeded = await onTest(application);
    if (testRequestRef.current !== requestId) return;
    setTestState(succeeded ? "success" : "error");
  };

  const testLabel = testState === "loading"
    ? labels.opening
    : testState === "success"
      ? labels.opened
      : testState === "error"
        ? labels.failed
        : labels.test;

  return (
    <>
      <div className="settingsApplicationRow">
        <div className="settingsApplicationInput">
          <input
            id={id}
            className="settingsSelect"
            aria-label={ariaLabel}
            placeholder={placeholder}
            value={displayValue}
            onChange={(event) => {
              onChange(event.target.value);
              resetTestState();
            }}
            onBlur={() => {
              void onSave(value);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
              if (event.key === "Escape") {
                onChange(savedValue);
                onCancel();
                resetTestState();
              }
            }}
          />
          <DropdownMenu.Root open={menuOpen} onOpenChange={setMenuOpen}>
            <DropdownMenu.Trigger asChild>
              <IconButton className="settingsApplicationMenuButton" aria-label={menuAriaLabel} type="button">
                <ChevronDown size={14} />
              </IconButton>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content className="skillMenuContent settingsSelectContent" align="end" sideOffset={6}>
                {options.map((option) => (
                  <DropdownMenu.Item
                    className="skillMenuItem"
                    key={option.value}
                    onSelect={() => chooseOption(option.value)}
                  >
                    {option.available === false ? `${option.label} (not found)` : option.label}
                  </DropdownMenu.Item>
                ))}
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>
        <StatefulButton
          className="settingsApplicationTestButton"
          size="sm"
          width={38}
          minWidth={38}
          state={testState}
          aria-label={testLabel}
          disabled={!value.trim()}
          onClick={() => { void testApplication(); }}
          loadingContent={<LoadingIcon size={14} />}
          successContent={<Check size={14} aria-hidden="true" />}
          errorContent={<AlertCircle size={14} aria-hidden="true" />}
        >
          <Play size={14} aria-hidden="true" />
        </StatefulButton>
      </div>
      {error ? <span className="settingsError" role="alert">{error}</span> : null}
    </>
  );
}
