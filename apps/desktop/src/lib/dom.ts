/** Swallow the ghost click that lands on the element under a just-closed Radix menu. */
export function suppressNextClick(ms = 400) {
  const until = performance.now() + ms;
  const onClick = (event: MouseEvent) => {
    if (performance.now() > until) {
      document.removeEventListener("click", onClick, true);
      return;
    }
    event.preventDefault();
    event.stopImmediatePropagation();
    document.removeEventListener("click", onClick, true);
  };
  document.addEventListener("click", onClick, true);
}
