import type { Metadata } from "next";
import { Archivo, Courier_Prime } from "next/font/google";
import "./globals.css";

// Printed form furniture: one grotesque whose width axis does the work of a form's
// condensed box labels and its heavy form-number block.
const formPrint = Archivo({
  subsets: ["latin"],
  axes: ["wdth"],
  variable: "--font-print",
  display: "swap",
});

// Values typed onto the form, the way tax software prints its entries.
const typedEntry = Courier_Prime({
  subsets: ["latin"],
  weight: ["400", "700"],
  variable: "--font-typed",
  display: "swap",
});

export const metadata: Metadata = {
  title: "Pru | Consent-checked AI for tax preparers",
  description: "A judge-facing test-data demonstration of consent-aware model routing for tax preparers.",
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en" className={`${formPrint.variable} ${typedEntry.variable}`}>
      <body>{children}</body>
    </html>
  );
}
