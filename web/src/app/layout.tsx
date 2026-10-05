import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Pru | Private AI consent boundary",
  description: "A judge-facing test-data demonstration of consent-aware model routing.",
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
