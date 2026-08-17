import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";

export type IconButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> & {
  children: ReactNode;
};

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { className = "", type = "button", children, ...props },
  ref,
) {
  return (
    <button ref={ref} type={type} {...props} className={`iconButton${className ? ` ${className}` : ""}`}>
      {children}
    </button>
  );
});
