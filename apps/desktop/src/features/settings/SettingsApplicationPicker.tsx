import { AlertCircle, Check, Play } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { LoadingIcon } from "../../components/shared/LoadingIcon.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { StatefulButton } from "../../components/shared/StatefulButton.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { AsyncStatus } from "../../lib/async-status.ts";

export type SettingsApplicationOption = {
  value: string;
  label: string;
  available?: boolean;
};

type TestState = AsyncStatus;

function displayValueForOption(options: SettingsApplicationOption[], value: string): string {
  const selectedOption = options.find((option) => option.value === value);
  return selectedOption
    ? `${selectedOption.label}${selectedOption.available === false ? " (not found)" : ""}`
    : value;
}

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
  const [testState, setTestState] = useState<TestState>(AsyncStatus.Idle);
  const [inputValue, setInputValue] = useState(() => displayValueForOption(options, value));
  const lastValueRef = useRef(value);
  const localValueRef = useRef(value);
  const testRequestRef = useRef(0);

  useEffect(() => {
    if (value === lastValueRef.current) return;
    lastValueRef.current = value;
    if (value === localValueRef.current) return;
    localValueRef.current = value;
    setInputValue(displayValueForOption(options, value));
  }, [options, value]);

  const resetTestState = () => {
    testRequestRef.current += 1;
    setTestState(AsyncStatus.Idle);
  };

  const chooseOption = (nextValue: string) => {
    localValueRef.current = nextValue;
    setInputValue(displayValueForOption(options, nextValue));
    onChange(nextValue);
    resetTestState();
    void onSave(nextValue);
  };

  const testApplication = async () => {
    const application = value.trim();
    if (!application) return;
    const requestId = testRequestRef.current + 1;
    testRequestRef.current = requestId;
    setTestState(AsyncStatus.Loading);
    const succeeded = await onTest(application);
    if (testRequestRef.current !== requestId) return;
    setTestState(succeeded ? AsyncStatus.Success : AsyncStatus.Error);
  };

  const testLabel = testState === AsyncStatus.Loading
    ? labels.opening
    : testState === AsyncStatus.Success
      ? labels.opened
      : testState === AsyncStatus.Error
        ? labels.failed
        : labels.test;

  return (
    <>
      <div className="settingsApplicationRow">
        <SelectControl
          variant="editable"
          className="settingsApplicationSelect"
          contentClassName="settingsSelectContent"
          label={ariaLabel}
          value={value}
          onValueChange={chooseOption}
          options={options}
          inputId={id}
          inputAriaLabel={ariaLabel}
          inputPlaceholder={placeholder}
          menuAriaLabel={menuAriaLabel}
          inputValue={inputValue}
          onInputChange={(nextValue) => {
            localValueRef.current = nextValue;
            setInputValue(nextValue);
            onChange(nextValue);
            resetTestState();
          }}
          onInputBlur={() => {
            void onSave(value);
          }}
          onInputKeyDown={(event) => {
            if (event.key === "Enter") event.currentTarget.blur();
            if (event.key === "Escape") {
              localValueRef.current = savedValue;
              setInputValue(displayValueForOption(options, savedValue));
              onChange(savedValue);
              onCancel();
              resetTestState();
            }
          }}
          showOptionTooltip={false}
        />
        <StatefulButton
          size="sm"
          variant="primary"
          iconOnly
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
      {error ? <Toast tone="error" message={error} /> : null}
    </>
  );
}
