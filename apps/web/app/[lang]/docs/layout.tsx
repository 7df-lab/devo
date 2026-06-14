import type { ReactNode } from "react";
import { notFound } from "next/navigation";
import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { RootProvider } from "fumadocs-ui/provider/next";
import { DocsLanguageSwitch } from "@/app/_components/docs-language-switch";
import { DevoWord } from "@/app/_components/landing/devo-word";
import { i18n } from "@/lib/i18n";
import { docsI18nProvider } from "@/lib/layout.shared";
import { source } from "@/lib/source";

export default async function Layout({
  params,
  children,
}: {
  params: Promise<{ lang: string }>;
  children: ReactNode;
}) {
  const { lang } = await params;

  if (!i18n.languages.includes(lang as (typeof i18n.languages)[number])) {
    notFound();
  }

  return (
    <RootProvider i18n={docsI18nProvider(lang)} theme={{ enabled: false }}>
      <DocsLayout
        i18n={false}
        themeSwitch={{ enabled: false }}
        tree={source.getPageTree(lang)}
        nav={{
          title: (
            <DevoWord
              className="text-sm font-semibold"
              iconClassName="h-5 w-5 rounded-full bg-[#070a0f] p-0.5"
              key="docs-brand"
            />
          ),
          children: (
            <DocsLanguageSwitch
              key="docs-language-switch"
              locale={lang as "en" | "zh"}
            />
          ),
        }}
      >
        {children}
      </DocsLayout>
    </RootProvider>
  );
}
