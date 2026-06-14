"use client";

import Image from "next/image";
import Link from "next/link";
import { useEffect, useRef, useState } from "react";

import {
  installCommands,
  type InstallId,
  type LandingCopy,
  type Locale,
} from "./data";
import { DevoWord, renderWithDevoMark } from "./devo-word";
import { ArrowIcon, CopyIcon } from "./icons";

const installTabs = ["unix", "windows", "source"] as const;

function HeaderBrand() {
  const brandRef = useRef<HTMLSpanElement>(null);
  const [attentionCycle, setAttentionCycle] = useState(0);

  useEffect(() => {
    const element = brandRef.current;

    if (!element) {
      return;
    }

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry?.isIntersecting) {
          setAttentionCycle((cycle) => cycle + 1);
        }
      },
      { threshold: 0.72 },
    );

    observer.observe(element);

    return () => observer.disconnect();
  }, []);

  const shouldAnimate = attentionCycle > 0;

  return (
    <span ref={brandRef} className="inline-flex items-center gap-2.5">
      <span className="inline-flex h-10 w-10 items-center justify-center">
        <Image
          src="/devo-mark.svg"
          alt=""
          width={36}
          height={36}
          className={[
            "h-9 w-9",
            shouldAnimate ? "devo-brand-mark-attention" : "",
          ].join(" ")}
          key={`brand-mark-${attentionCycle}`}
        />
      </span>
      <span className="sr-only">Devo</span>
      <span
        aria-hidden="true"
        className={[
          "devo-brand-word",
          shouldAnimate ? "devo-brand-word-attention" : "",
        ].join(" ")}
        key={`brand-word-${attentionCycle}`}
      >
        <span className="devo-brand-word-track">
          <span>DEVO</span>
          <span>devo</span>
          <span>Devo</span>
        </span>
      </span>
    </span>
  );
}

type HeroSectionProps = {
  copy: LandingCopy;
  locale: Locale;
  onLocaleChange: (locale: Locale) => void;
};

export function HeroSection({
  copy,
  locale,
  onLocaleChange,
}: HeroSectionProps) {
  const [activeInstall, setActiveInstall] = useState<InstallId>("unix");
  const [copied, setCopied] = useState(false);
  const [installAttentionCycle, setInstallAttentionCycle] = useState(0);
  const command = installCommands[activeInstall];

  async function copyInstallCommand() {
    await navigator.clipboard.writeText(command);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  }

  function highlightInstallCard() {
    setInstallAttentionCycle((cycle) => cycle + 1);
  }

  return (
    <section className="relative isolate min-h-[94vh] overflow-hidden md:min-h-[92vh]">
      <Image
        src="/devo-hero-halftone.png"
        alt=""
        fill
        priority
        sizes="100vw"
        className="object-cover object-[64%_center] md:object-center"
      />
      <div className="absolute inset-0 bg-[linear-gradient(90deg,rgba(7,10,15,0.98)_0%,rgba(7,10,15,0.88)_34%,rgba(7,10,15,0.38)_67%,rgba(7,10,15,0.1)_100%)]" />
      <div className="absolute inset-0 bg-[radial-gradient(circle_at_82%_58%,rgba(255,139,24,0.14),transparent_32%),linear-gradient(180deg,rgba(7,10,15,0)_55%,#070a0f_100%)]" />

      <header className="relative z-10 mx-auto flex w-full max-w-7xl flex-col gap-4 px-5 py-5 sm:flex-row sm:items-center sm:justify-between sm:px-8 lg:px-10">
        <Link
          href="/"
          className="inline-flex items-center gap-2.5 text-base font-bold tracking-normal text-white"
          aria-label="Devo home"
        >
          <HeaderBrand />
        </Link>
        <div className="grid w-full grid-cols-1 gap-2 sm:flex sm:w-auto sm:items-center">
          <nav className="grid grid-cols-4 items-center gap-1 rounded-full border border-white/12 bg-black/22 p-1 text-sm text-white/72 backdrop-blur-md sm:flex">
            <Link
              className="inline-flex min-h-10 items-center justify-center rounded-full px-3 transition-colors hover:bg-white/10 hover:text-white sm:min-h-9 sm:px-4"
              href={locale === "zh" ? "/zh/docs" : "/docs"}
            >
              {copy.nav.docs}
            </Link>
            <a
              className="inline-flex min-h-10 items-center justify-center rounded-full px-3 transition-colors hover:bg-white/10 hover:text-white sm:min-h-9 sm:px-4"
              onClick={highlightInstallCard}
            >
              {copy.nav.install}
            </a>
            <a
              className="inline-flex min-h-10 items-center justify-center rounded-full px-3 transition-colors hover:bg-white/10 hover:text-white sm:min-h-9 sm:px-4"
              href="#contact"
            >
              {copy.nav.contact}
            </a>
            <a
              className="inline-flex min-h-10 items-center justify-center rounded-full px-3 transition-colors hover:bg-white/10 hover:text-white sm:min-h-9 sm:px-4"
              href="https://github.com/7df-lab/devo"
              rel="noreferrer"
              target="_blank"
            >
              {copy.nav.github}
            </a>
          </nav>
          <div
            aria-label={copy.language.label}
            className="grid grid-cols-2 rounded-full border border-white/12 bg-black/22 p-1 text-xs font-bold text-white/64 backdrop-blur-md"
            role="group"
          >
            <button
              aria-pressed={locale === "en"}
              className="min-h-9 rounded-full px-3 transition-colors aria-pressed:bg-white/12 aria-pressed:text-white hover:bg-white/10 hover:text-white"
              onClick={() => onLocaleChange("en")}
              type="button"
            >
              {copy.language.en}
            </button>
            <button
              aria-pressed={locale === "zh"}
              className="min-h-9 rounded-full px-3 transition-colors aria-pressed:bg-white/12 aria-pressed:text-white hover:bg-white/10 hover:text-white"
              onClick={() => onLocaleChange("zh")}
              type="button"
            >
              {copy.language.zh}
            </button>
          </div>
        </div>
      </header>

      <div className="relative z-10 mx-auto grid min-h-[calc(92vh-84px)] w-full max-w-7xl items-center gap-12 px-5 pb-16 pt-8 sm:px-8 lg:grid-cols-[minmax(0,0.82fr)_minmax(28rem,0.68fr)] lg:px-10">
        <div className="max-w-3xl">
          <h1 className="text-[clamp(3.6rem,10vw,8.8rem)] font-semibold leading-[0.84] tracking-normal text-white">
            <DevoWord
              className="gap-[0.16em]"
              iconClassName="h-[0.66em] w-[0.66em]"
            />
          </h1>
          <p className="mt-6 max-w-2xl text-balance text-xl leading-8 text-white/76 sm:text-2xl sm:leading-9">
            {copy.hero.body}
          </p>
          <div className="mt-9 flex flex-col gap-3 sm:flex-row">
            <a
              className="inline-flex min-h-12 items-center justify-center gap-2 bg-[#ff941f] px-5 text-sm font-bold text-[#080a0e] transition hover:-translate-y-px hover:bg-[#ffb45f]"
              onClick={highlightInstallCard}
            >
              {renderWithDevoMark(copy.hero.primaryCta)}
              <ArrowIcon />
            </a>
            <Link
              className="inline-flex min-h-12 items-center justify-center gap-2 border border-white/20 bg-white/7 px-5 text-sm font-bold text-white transition hover:-translate-y-px hover:border-white/35 hover:bg-white/12"
              href={locale === "zh" ? "/zh/docs" : "/docs"}
            >
              {copy.hero.secondaryCta}
            </Link>
          </div>
        </div>

        <div
          id="install"
          className={[
            "relative w-full overflow-hidden border border-white/14 bg-white/[0.055] shadow-[0_2rem_6rem_rgb(0_0_0_/_32%)] backdrop-blur-2xl",
            installAttentionCycle > 0 ? "devo-install-card-attention" : "",
          ].join(" ")}
          key={`install-card-${installAttentionCycle}`}
        >
          <div
            aria-label={copy.install.tabAria}
            className="flex gap-1 overflow-x-auto border-b border-white/10 bg-black/18 p-1.5"
            role="tablist"
          >
            {installTabs.map((tab) => (
              <button
                aria-selected={activeInstall === tab}
                className="min-h-11 whitespace-nowrap px-4 text-sm font-bold text-white/50 transition-colors aria-selected:bg-[#60A5FA]/16 aria-selected:text-[#bfdbfe] hover:bg-white/8 hover:text-white"
                key={tab}
                onClick={() => setActiveInstall(tab)}
                role="tab"
                type="button"
              >
                {copy.install.tabs[tab]}
              </button>
            ))}
          </div>
          <div className="flex min-h-14 items-center justify-between gap-4 border-b border-white/10 px-4 font-mono text-sm text-white/48">
            <span>{copy.install.terminalTitle}</span>
            <button
              className="devo-install-copy-button inline-flex min-h-9 items-center gap-2 border border-white/14 bg-white/[0.045] px-3 font-sans text-sm font-bold text-white transition hover:border-[#60A5FA]/45 hover:bg-[#60A5FA]/12"
              onClick={copyInstallCommand}
              type="button"
            >
              <CopyIcon />
              {copied ? copy.install.copied : copy.install.copy}
            </button>
          </div>
          <pre className="devo-install-command min-h-44 overflow-x-auto whitespace-pre-wrap break-words bg-black/22 p-5 font-mono text-[clamp(0.88rem,1.35vw,1.02rem)] leading-8 text-white/90 sm:min-h-52 sm:p-6">
            <code>{command}</code>
          </pre>
        </div>
      </div>
    </section>
  );
}
