import { Check } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import type { ReactNode } from "react";
import { cn } from "./cn";

/** A group of menu rows of which exactly one is chosen (a view setting): the chosen row carries a check. Use inside `Menu`. */
export function MenuRadioGroup<T extends string>({
  value,
  onValueChange,
  label,
  children,
}: {
  value: T;
  onValueChange: (v: T) => void;
  label: string;
  children: ReactNode;
}) {
  return (
    <DropdownMenu.RadioGroup value={value} onValueChange={(v) => onValueChange(v as T)} aria-label={label}>
      {children}
    </DropdownMenu.RadioGroup>
  );
}

export function MenuRadioItem({ value, children, icon }: { value: string; children: ReactNode; icon?: ReactNode }) {
  return (
    <DropdownMenu.RadioItem
      value={value}
      className={cn(
        "flex min-h-10 cursor-default items-center gap-3 rounded-[0.55rem] px-2.5 text-[0.9375rem] outline-none select-none",
        "data-[highlighted]:bg-surface [&_svg]:size-[1.1rem] [&_svg]:shrink-0 [&_svg]:text-muted",
      )}
    >
      {icon}
      <span className="flex-1">{children}</span>
      <DropdownMenu.ItemIndicator>
        <Check className="text-accent!" aria-hidden="true" />
      </DropdownMenu.ItemIndicator>
    </DropdownMenu.RadioItem>
  );
}
