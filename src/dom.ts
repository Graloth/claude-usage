/** The element with `id`, or a loud failure: a missing id is a programming error. */
export function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing #${id}`);
  return el as T;
}

type Child = Node | string;

/**
 * Builds an element. Text always goes in as text nodes, never as HTML: some of
 * it (such as model names) comes from the server.
 */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (className) el.className = className;
  el.append(...children);
  return el;
}
