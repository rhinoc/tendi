import type { ReactNode } from "react";

export function SettingsSection({ title, children, className = "" }: { title: string; children: ReactNode; className?: string }) {
  return (
    <section className={`settingsSection ${className}`.trim()}>
      <div className="settingsSectionText">
        <h2>{title}</h2>
      </div>
      <div className="settingsControlGroup">{children}</div>
    </section>
  );
}
