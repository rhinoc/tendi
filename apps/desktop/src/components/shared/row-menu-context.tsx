import { createContext, useContext, type ReactNode } from "react";

type RowMenuOpenChange = (open: boolean) => void;

const RowMenuOpenChangeContext = createContext<RowMenuOpenChange | null>(null);

export function RowMenuOpenChangeProvider({ onOpenChange, children }: { onOpenChange: RowMenuOpenChange; children: ReactNode }) {
  return (
    <RowMenuOpenChangeContext.Provider value={onOpenChange}>
      {children}
    </RowMenuOpenChangeContext.Provider>
  );
}

export function useRowMenuOpenChange() {
  return useContext(RowMenuOpenChangeContext);
}
