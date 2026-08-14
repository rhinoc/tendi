import type { ReactNode } from "react";

export type InfoSectionProps = {
  label: ReactNode;
  children: ReactNode;
  valueLine?: boolean;
  valueClassName?: string;
  className?: string;
};

export function InfoSection({ label, children, valueLine = true, valueClassName = "skillInfoValueLine", className = "" }: InfoSectionProps) {
  return (
    <section className={`skillInfoSection${className ? ` ${className}` : ""}`}>
      <span>{label}</span>
      {valueLine ? <div className={valueClassName}>{children}</div> : children}
    </section>
  );
}
