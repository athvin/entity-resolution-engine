import { ArrowDown, ArrowRight, Database, FileSpreadsheet, Globe, Layers } from "lucide-react";

const SYSTEMS = [
  { icon: Database, name: "Databases & warehouses" },
  { icon: FileSpreadsheet, name: "Files & spreadsheets" },
  { icon: Globe, name: "APIs & custom systems" },
  { icon: Layers, name: "Business applications" },
];
export function Connections() {
  return (
    <section id="connections" className="scroll-mt-20 border-t" data-testid="marketing-connections">
      <div className="mx-auto max-w-5xl px-6 py-20 lg:py-28">
        <div className="max-w-2xl">
          <p className="text-muted-foreground mb-4 text-xs font-medium tracking-widest uppercase">
            Connections, in both directions
          </p>
          <h2 className="text-2xl font-medium tracking-tight sm:text-3xl">
            Keep your systems. Connect the records.
          </h2>
          <p className="text-muted-foreground mt-4 leading-relaxed">
            Your stack is the starting point. We build the source connections and destination
            writes, including custom integrations, around the way your team works.
          </p>
        </div>
        <div className="mt-12 grid items-center gap-6 sm:grid-cols-[1fr_auto_1fr]">
          <div className="border-y py-5">
            <h3 className="mb-5 text-sm font-medium">Bring data from</h3>
            <ul className="space-y-4">
              {SYSTEMS.map(({ icon: Icon, name }) => (
                <li className="flex items-center gap-3 text-sm" key={name}>
                  <Icon size={16} aria-hidden="true" />
                  {name}
                </li>
              ))}
            </ul>
          </div>
          <div className="flex items-center justify-center gap-3 py-4 text-sm font-medium">
            <ArrowRight className="hidden sm:block" size={16} aria-hidden="true" />
            <ArrowDown className="sm:hidden" size={16} aria-hidden="true" />
            DupeZero
            <ArrowRight className="hidden sm:block" size={16} aria-hidden="true" />
            <ArrowDown className="sm:hidden" size={16} aria-hidden="true" />
          </div>
          <div className="border-y py-5">
            <h3 className="mb-5 text-sm font-medium">Deliver resolved records to</h3>
            <ul className="space-y-4">
              {[...SYSTEMS].reverse().map(({ icon: Icon, name }) => (
                <li className="flex items-center gap-3 text-sm" key={name}>
                  <Icon size={16} aria-hidden="true" />
                  {name}
                </li>
              ))}
            </ul>
          </div>
        </div>
        <p className="text-muted-foreground mt-6 text-sm">
          Source access, field mapping, output format, and write behavior are scoped to your
          integration.
        </p>
      </div>
    </section>
  );
}
