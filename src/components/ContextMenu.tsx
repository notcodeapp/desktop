import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

export interface ContextMenuItem {
  id: string;
  label: string;
  hint?: string;
  danger?: boolean;
  disabled?: boolean;
}

interface Payload { x: number; y: number; items: ContextMenuItem[]; onSelect: (id: string) => void }

/// Themed right-click menu (VS Code renders its own too — it is what makes
/// context menus feel consistent instead of "website"). Closes on click
/// outside, Escape, or selection; flips at screen edges.
export function ContextMenuPortal({ payload }: { payload: Payload | null }) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ x: 0, y: 0 });
  useEffect(() => {
    if (!payload) return;
    const menu = ref.current;
    const width = menu?.offsetWidth ?? 220;
    const height = menu?.offsetHeight ?? 200;
    setPosition({
      x: Math.min(payload.x, window.innerWidth - width - 8),
      y: Math.min(payload.y, window.innerHeight - height - 8),
    });
  }, [payload]);
  useEffect(() => {
    if (!payload) return;
    function onDown(event: PointerEvent) {
      if (!ref.current?.contains(event.target as Node)) payload?.onSelect('');
    }
    function onKey(event: KeyboardEvent) { if (event.key === 'Escape') payload?.onSelect(''); }
    document.addEventListener('pointerdown', onDown);
    document.addEventListener('keydown', onKey);
    return () => { document.removeEventListener('pointerdown', onDown); document.removeEventListener('keydown', onKey); };
  }, [payload]);
  if (!payload) return null;
  return createPortal(<div ref={ref} className="context-menu" role="menu" style={{ left: position.x, top: position.y }}>
    {payload.items.map(item => <button
      key={item.id}
      type="button"
      role="menuitem"
      className={`context-item ${item.danger ? 'danger' : ''}`}
      disabled={item.disabled}
      onClick={() => payload.onSelect(item.id)}
    ><span>{item.label}</span>{item.hint && <kbd>{item.hint}</kbd>}</button>)}
  </div>, document.body);
}
