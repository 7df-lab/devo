import type { ReactNode } from "react";
import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { RootProvider } from "fumadocs-ui/provider/next";
import { DocsLanguageSwitch } from "@/app/_components/docs-language-switch";
import { DevoWord } from "@/app/_components/landing/devo-word";
import { docsI18nProvider } from "@/lib/layout.shared";
import { source } from "@/lib/source";

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <RootProvider i18n={docsI18nProvider("en")} theme={{ enabled: false }}>
      <DocsLayout
        i18n={false}
        themeSwitch={{ enabled: false }}
        tree={source.getPageTree("en")}
        nav={{
          title: (
            <DevoWord
              className="text-sm font-semibold"
              iconClassName="h-5 w-5 rounded-full bg-[#070a0f] p-0.5"
              key="docs-brand"
            />
          ),
          children: <DocsLanguageSwitch key="docs-language-switch" locale="en" />,
        }}
      >
        {children}
      </DocsLayout>
    </RootProvider>
  );
}
