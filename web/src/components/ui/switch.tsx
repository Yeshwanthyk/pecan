import { Switch as BaseSwitch } from "@base-ui/react/switch";
import type { ComponentProps } from "react";

import { cn } from "~/lib/utils";

export function Switch({ className, ...props }: ComponentProps<typeof BaseSwitch.Root>) {
  return (
    <BaseSwitch.Root
      className={cn(
        "group relative inline-flex h-6 w-10 shrink-0 cursor-pointer items-center rounded-full bg-muted-foreground/25 p-0.5 outline-none transition-colors data-[checked]:bg-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-not-allowed disabled:opacity-40",
        className,
      )}
      {...props}
    >
      <BaseSwitch.Thumb className="block size-5 rounded-full bg-background shadow-sm transition-transform duration-150 group-data-[checked]:translate-x-4" />
    </BaseSwitch.Root>
  );
}
