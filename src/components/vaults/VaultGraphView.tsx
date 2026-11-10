import { useState } from "react";
import type { VaultGraph } from "../../lib/api";
import { noteTitle } from "./shared";

export function VaultGraphView({
  graph,
  onOpen,
}: {
  graph: VaultGraph;
  onOpen: (id: string) => void;
}) {
  const [hovered, setHovered] = useState<string | null>(null);
  const width = 760;
  const height = 560;
  const centerX = width / 2;
  const centerY = height / 2;
  const radius = Math.min(width, height) * 0.36;
  const count = graph.nodes.length;

  const positions = new Map<string, { x: number; y: number }>();
  graph.nodes.forEach((node, index) => {
    if (count === 1) {
      positions.set(node.id, { x: centerX, y: centerY });
      return;
    }
    const angle = (Math.PI * 2 * index) / count - Math.PI / 2;
    positions.set(node.id, {
      x: centerX + Math.cos(angle) * radius,
      y: centerY + Math.sin(angle) * radius,
    });
  });

  const activeNode =
    hovered && graph.nodes.some((node) => node.id === hovered) ? hovered : null;

  const highlighted = new Set<string>();
  if (activeNode) {
    highlighted.add(activeNode);
    for (const edge of graph.edges) {
      if (edge.from === activeNode) highlighted.add(edge.to);
      if (edge.to === activeNode) highlighted.add(edge.from);
    }
  }

  return (
    <svg
      viewBox={`0 0 ${width} ${height}`}
      className="h-full w-full"
      onMouseLeave={() => setHovered(null)}
    >
      <g>
        {graph.edges.map((edge, index) => {
          const from = positions.get(edge.from);
          const to = positions.get(edge.to);
          if (!from || !to) return null;
          const active =
            activeNode !== null &&
            (edge.from === activeNode || edge.to === activeNode);
          return (
            <line
              key={`${edge.from}->${edge.to}:${index}`}
              x1={from.x}
              y1={from.y}
              x2={to.x}
              y2={to.y}
              stroke="var(--border)"
              strokeWidth={active ? 1.4 : 0.8}
              opacity={activeNode ? (active ? 0.9 : 0.2) : 0.55}
            />
          );
        })}
        {graph.nodes.map((node) => {
          const position = positions.get(node.id);
          if (!position) return null;
          const isHovered = activeNode === node.id;
          const dimmed = activeNode !== null && !highlighted.has(node.id);
          const label =
            activeNode === null || count <= 60 || highlighted.has(node.id);
          const name = node.title || noteTitle(node.path);
          const shortName = name.length > 18 ? `${name.slice(0, 18)}…` : name;
          return (
            <g
              key={node.id}
              transform={`translate(${position.x} ${position.y})`}
              opacity={dimmed ? 0.3 : 1}
              style={{ cursor: "pointer" }}
              onMouseEnter={() => setHovered(node.id)}
              onClick={() => onOpen(node.id)}
            >
              <title>{node.path}</title>
              <circle r={isHovered ? 10 : 7} fill="var(--accent)" fillOpacity={0.15} />
              <circle r={isHovered ? 6 : 4.5} fill="var(--accent)" />
              {label && (
                <text
                  y={isHovered ? -14 : 15}
                  textAnchor="middle"
                  style={{ fill: "var(--muted)", fontSize: 10 }}
                  className="select-none"
                >
                  {shortName}
                </text>
              )}
            </g>
          );
        })}
      </g>
    </svg>
  );
}
