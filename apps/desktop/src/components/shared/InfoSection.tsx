import type { ReactNode } from "react";

export type InfoSectionProps = {
  label: ReactNode;
  children: ReactNode;
  valueLine?: boolean;
  valueClassName?: string;
  className?: string;
};

export function InfoSection({ label, children, valueLine = true, valueClassName = "infoValueLine", className = "" }: InfoSectionProps) {
  return (
    <section className={`infoSection${className ? ` ${className}` : ""}`}>
      <span>{label}</span>
      {valueLine ? <div className={valueClassName}>{children}</div> : children}
    </section>
  );
}
