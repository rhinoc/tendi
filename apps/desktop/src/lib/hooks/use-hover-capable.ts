import { useEffect, useState } from "react";

export function useHoverCapable() {
  const [canHover, setCanHover] = useState(false);

  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mediaQuery = window.matchMedia("(hover: hover) and (pointer: fine)");
    const update = () => setCanHover(mediaQuery.matches);
    update();
    mediaQuery.addEventListener?.("change", update);
    return () => mediaQuery.removeEventListener?.("change", update);
  }, []);

  return canHover;
}
