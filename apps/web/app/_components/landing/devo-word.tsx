import Image from "next/image";
import { type ReactNode } from "react";

type DevoWordProps = {
  className?: string;
  iconClassName?: string;
};

type RenderWithDevoMarkOptions = {
  iconClassName?: string;
  wordClassName?: string;
};

export function DevoWord({
  className = "",
  iconClassName = "h-[0.95em] w-[0.95em]",
}: DevoWordProps) {
  return (
    <span
      className={[
        "inline-flex items-center gap-[0.28em] whitespace-nowrap align-[-0.08em]",
        className,
      ].join(" ")}
    >
      <Image
        alt=""
        className={["shrink-0", iconClassName].join(" ")}
        height={24}
        src="/devo-mark.svg"
        width={24}
      />
      <span>Devo</span>
    </span>
  );
}

export function renderWithDevoMark(
  text: string,
  options: RenderWithDevoMarkOptions = {},
): ReactNode {
  if (!text.includes("Devo")) {
    return text;
  }

  return text.split("Devo").flatMap((part, index) => {
    if (index === 0) {
      return part ? [part] : [];
    }

    return [
      <DevoWord
        className={options.wordClassName}
        iconClassName={options.iconClassName}
        key={`devo-${index}`}
      />,
      part,
    ];
  });
}
