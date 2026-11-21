import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { X, ChevronDown, Check } from 'lucide-react';
export function Modal({ title, children, close, wide = false }: {title: string; children: ReactNode; close: () => void; wide?: boolean}) {
  const ref = useRef<HTMLDivElement>(null); const label = useId();
  const closeRef = useRef(close); closeRef.current = close;
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const node = ref.current;
    const focusable = () => Array.from(node?.querySelectorAll<HTMLElement>('button:not(:disabled),input,textarea,select,[tabindex="0"]') || []);
    (focusable()[0] || node)?.focus();
    function keys(e: KeyboardEvent) {
      if (e.key === 'Escape') { e.stopPropagation(); closeRef.current(); }
      if (e.key !== 'Tab') return;
      const elements = focusable(); const first = elements[0]; const last = elements.at(-1);
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
    }
    node?.addEventListener('keydown', keys);
    return () => { node?.removeEventListener('keydown', keys); previous?.focus(); };
  }, []);
  return <div className="scrim"><div ref={ref} role="dialog" aria-modal="true" aria-labelledby={label} tabIndex={-1} className={`dialog ${wide ? 'dialog-wide' : ''}`}><header><h2 id={label}>{title}</h2><button className="icon-button" title="Закрыть" onClick={close}><X/></button></header>{children}</div></div>;
}
export function Empty({ title, children }: { title: string; children?: ReactNode }) {
  return <div className="empty"><h3>{title}</h3>{children && <div className="muted">{children}</div>}</div>;
}
export function SpinnerLabel({ children }: {children: ReactNode}) { return <span className="busy-label" role="status"><span className="busy-dot"/>{children}</span>; }
export type Confirm = (title: string, detail: string, allowRemember?: boolean, scope?: string) => Promise<boolean>;
export type Report = (error: unknown) => void;
export interface DropdownOption { value: string; label: string; hint?: string }
export function Dropdown({ value, options, onChange, disabled, label, placeholder = 'Выберите…', className = '', menuClassName = '' }: { value: string; options: DropdownOption[]; onChange: (value: string) => void; disabled?: boolean; label: string; placeholder?: string; className?: string; menuClassName?: string }) {
  const [open, setOpen] = useState(false); const [active, setActive] = useState(0);
  const root = useRef<HTMLDivElement>(null); const list = useRef<HTMLDivElement>(null); const id = useId();
  const selected = options.find(option => option.value === value);
  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);
  useEffect(() => { if (open) setActive(Math.max(0, options.findIndex(option => option.value === value))); }, [open, options, value]);
  useEffect(() => { if (open) (list.current?.children[active] as HTMLElement | undefined)?.scrollIntoView({ block: 'nearest' }); }, [active, open]);
  function move(delta: number) { setActive(current => options.length ? (current + delta + options.length) % options.length : 0); }
  function keys(e: React.KeyboardEvent) {
    if (disabled) return;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); if (!open) setOpen(true); else move(e.key === 'ArrowDown' ? 1 : -1); }
    else if (e.key === 'Home' && open) { e.preventDefault(); setActive(0); }
    else if (e.key === 'End' && open) { e.preventDefault(); setActive(options.length - 1); }
    else if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); if (!open) setOpen(true); else { const option = options[active]; if (option) onChange(option.value); setOpen(false); } }
    else if (e.key === 'Escape' && open) { e.stopPropagation(); setOpen(false); }
    else if (e.key === 'Tab') setOpen(false);
  }
  return <div className={`dropdown ${className} ${open ? 'open' : ''}`} ref={root}>
    <button type="button" className="dropdown-trigger" disabled={disabled} aria-haspopup="listbox" aria-expanded={open} aria-label={label} aria-activedescendant={open && options[active] ? `${id}-${active}` : undefined} title={selected?.label || placeholder} onClick={() => setOpen(v => !v)} onKeyDown={keys}>
      <span className="dropdown-value">{selected?.label || placeholder}</span><ChevronDown className="dropdown-chevron"/>
    </button>
    {open && <div className={`dropdown-menu ${menuClassName}`} role="listbox" aria-label={label} ref={list}>
      {!options.length && <div className="dropdown-empty">Нет доступных вариантов</div>}
      {options.map((option, index) => <button type="button" key={option.value || `empty-${index}`} id={`${id}-${index}`} role="option" aria-selected={option.value === value} className={`dropdown-option ${index === active ? 'active' : ''} ${option.value === value ? 'selected' : ''}`} onMouseEnter={() => setActive(index)} onClick={() => { onChange(option.value); setOpen(false); }}>
        <span className="dropdown-option-text"><span>{option.label || placeholder}</span>{option.hint && <small>{option.hint}</small>}</span>{option.value === value && <Check className="dropdown-check"/>}
      </button>)}
    </div>}
  </div>;
}
