import { forwardRef, type ReactNode } from "react";

import { Button, type ButtonProps } from "./Button.tsx";

export type IconButtonProps = Omit<ButtonProps, "children" | "variant" | "iconOnly"> & {
  children: ReactNode;
};

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { className = "", type = "button", children, ...props },
  ref,
) {
  return (
    <Button ref={ref} type={type} {...props} variant="icon" iconOnly className={className}>
      {children}
    </Button>
  );
});
