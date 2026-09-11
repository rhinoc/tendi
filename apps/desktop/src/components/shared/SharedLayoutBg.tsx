import {
  AnimatePresence,
  motion,
  useReducedMotion,
  type HTMLMotionProps,
  type Variants,
} from "motion/react";
import {
  Children,
  cloneElement,
  forwardRef,
  isValidElement,
  useId,
  useState,
  type HTMLAttributes,
  type MouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";

import { SPRING_LAYOUT } from "../../lib/ease.ts";
import "./SharedLayoutBg.css";

const variants: Variants = {
  initial: { opacity: 0, filter: "blur(6px)" },
  animate: { opacity: 1, filter: "blur(0px)" },
  exit: (isActive: boolean) => (!isActive ? { opacity: 0, filter: "blur(6px)" } : {}),
};

const reducedVariants: Variants = {
  initial: { opacity: 0 },
  animate: { opacity: 1 },
  exit: (isActive: boolean) => (!isActive ? { opacity: 0 } : {}),
};

type SharedLayoutBgChildProps = {
  className?: string;
  onMouseEnter?: () => void;
  children?: ReactNode;
};

export type SharedLayoutBgProps = Omit<HTMLAttributes<HTMLElement>, "children"> & {
  children: ReactNode;
  as?: "div" | "ul";
  pillClassName?: string;
  inset?: number;
  pillContainerClassName?: string;
};

function joinClasses(...values: Array<string | undefined>) {
  return values.filter(Boolean).join(" ");
}

export const SharedLayoutBg = forwardRef<HTMLDivElement, SharedLayoutBgProps>(function SharedLayoutBg(
  {
    children,
    as = "div",
    className,
    onMouseLeave,
    pillClassName,
    pillContainerClassName,
    inset = 20,
    ...props
  },
  forwardedRef,
) {
  const [activeId, setActiveId] = useState<string | null>(null);
  const uid = useId();
  const reduce = useReducedMotion() ?? false;

  const renderedChildren = Children.toArray(children)
    .filter(isValidElement)
    .map((child, index) => {
      const element = child as ReactElement<SharedLayoutBgChildProps>;
      const childKey = element.key ? String(element.key) : `item-${index}`;
      return cloneElement(
        element,
        {
          key: childKey,
          className: joinClasses("sharedLayoutBgItem", element.props.className),
          onMouseEnter: () => {
            element.props.onMouseEnter?.();
            setActiveId(childKey);
          },
        },
        <>
          <AnimatePresence custom={activeId !== null}>
            {activeId !== null ? (
              <motion.div
                variants={reduce ? reducedVariants : variants}
                initial="initial"
                animate="animate"
                exit="exit"
                custom={activeId !== null}
                className={joinClasses("sharedLayoutBgPillContainer", pillContainerClassName)}
                style={{ left: -inset, right: -inset }}
              >
                {activeId === childKey ? (
                  <motion.div
                    layoutId={`shared-bg-${uid}`}
                    transition={reduce ? { duration: 0 } : SPRING_LAYOUT}
                    className={joinClasses("sharedLayoutBgPill", pillClassName)}
                  />
                ) : null}
              </motion.div>
            ) : null}
          </AnimatePresence>
          <div className="sharedLayoutBgContent">{element.props.children}</div>
        </>,
      );
    });

  const handleMouseLeave = (event: MouseEvent<HTMLElement>) => {
    setActiveId(null);
    onMouseLeave?.(event);
  };
  const motionDivProps = props as Omit<HTMLMotionProps<"div">, "ref">;
  const motionListProps = props as Omit<HTMLMotionProps<"ul">, "ref">;

  return as === "ul" ? (
    <motion.ul
      {...motionListProps}
      ref={forwardedRef as React.Ref<HTMLUListElement>}
      layoutRoot
      onMouseLeave={handleMouseLeave}
      className={joinClasses("sharedLayoutBg", className)}
    >
      {renderedChildren}
    </motion.ul>
  ) : (
    <motion.div
      {...motionDivProps}
      ref={forwardedRef}
      layoutRoot
      onMouseLeave={handleMouseLeave}
      className={joinClasses("sharedLayoutBg", className)}
    >
      {renderedChildren}
    </motion.div>
  );
});
