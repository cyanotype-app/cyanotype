import { motion } from "motion/react";
import { Switch as RSwitch, ToggleGroup } from "radix-ui";
import { useId, type ReactNode } from "react";
import { cn } from "./cn";

/** A segmented control ("Square | Justified"): one of a few options, the chosen one on a sliding pill. */
export function Segmented<T extends string>({
  value,
  onChange,
  options,
  label,
  className,
}: {
  value: T;
  onChange: (v: T) => void;
  options: readonly { value: T; label: ReactNode }[];
  label: string;
  className?: string;
}) {
  const layoutId = useId();
  return (
    <ToggleGroup.Root
      type="single"
      value={value}
      onValueChange={(v) => v && onChange(v as T)}
      aria-label={label}
      className={cn("inline-flex rounded-full bg-surface p-1", className)}
    >
      {options.map((o) => (
        <ToggleGroup.Item
          key={o.value}
          value={o.value}
          className="relative h-8 rounded-full px-3.5 text-[0.8125rem] font-medium text-muted transition-colors data-[state=on]:text-fg"
        >
          {o.value === value && (
            <motion.span
              layoutId={layoutId}
              className="absolute inset-0 rounded-full bg-card shadow-[0_1px_2px_rgb(0_0_0/0.12)]"
              transition={{ type: "spring", stiffness: 600, damping: 45 }}
            />
          )}
          <span className="relative">{o.label}</span>
        </ToggleGroup.Item>
      ))}
    </ToggleGroup.Root>
  );
}

/** An on/off switch with its label, the whole row tappable. */
export function Switch({
  checked,
  onChange,
  label,
  hint,
  disabled,
}: {
  checked: boolean;
  onChange: (on: boolean) => void;
  label: ReactNode;
  hint?: ReactNode;
  disabled?: boolean;
}) {
  const id = useId();
  return (
    <div className="flex items-center gap-4 py-2">
      <label htmlFor={id} className="min-w-0 flex-1 cursor-pointer">
        <span className="block text-[0.9375rem]">{label}</span>
        {hint && <span className="block text-sm text-muted">{hint}</span>}
      </label>
      {/* Inside a form Radix adds a hidden input beside the switch; this keeps it next to the switch, not off the page's edge. */}
      <span className="relative inline-flex shrink-0">
        <RSwitch.Root
          id={id}
          checked={checked}
          onCheckedChange={onChange}
          disabled={disabled}
          className="relative h-7 w-12 shrink-0 rounded-full bg-line transition-colors duration-200 data-[state=checked]:bg-accent disabled:opacity-45"
        >
          <RSwitch.Thumb className="block size-6 translate-x-0.5 rounded-full bg-white shadow-[0_1px_3px_rgb(0_0_0/0.25)] transition-transform duration-200 ease-soft data-[state=checked]:translate-x-[1.375rem]" />
        </RSwitch.Root>
      </span>
    </div>
  );
}
