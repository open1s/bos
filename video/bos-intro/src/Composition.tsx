import {
  AbsoluteFill,
  Easing,
  interpolate,
  Sequence,
  useCurrentFrame,
} from "remotion";

const TitleSlide: React.FC = () => {
  const frame = useCurrentFrame();

  const titleOpacity = interpolate(frame, [0, 20], [0, 1], {
    extrapolateRight: "clamp",
    easing: Easing.bezier(0.16, 1, 0.3, 1),
  });

  const titleScale = interpolate(frame, [0, 20], [0.8, 1], {
    extrapolateRight: "clamp",
    easing: Easing.bezier(0.16, 1, 0.3, 1),
  });

  const subtitleOpacity = interpolate(frame, [15, 40], [0, 1], {
    extrapolateRight: "clamp",
  });

  const taglineY = interpolate(frame, [15, 40], [20, 0], {
    extrapolateRight: "clamp",
    easing: Easing.bezier(0.16, 1, 0.3, 1),
  });

  return (
    <AbsoluteFill
      style={{
        backgroundColor: "#0f172a",
        justifyContent: "center",
        alignItems: "center",
        fontFamily: "Arial, sans-serif",
      }}
    >
      <div
        style={{
          opacity: titleOpacity,
          transform: `scale(${titleScale})`,
          textAlign: "center",
        }}
      >
        <div
          style={{
            fontSize: 120,
            fontWeight: 900,
            color: "#ffffff",
            letterSpacing: "-4px",
            textShadow: "0 0 60px rgba(99, 102, 241, 0.5)",
          }}
        >
          BrainOS
        </div>
        <div
          style={{
            fontSize: 28,
            color: "#94a3b8",
            marginTop: 16,
            letterSpacing: "2px",
          }}
        >
          MULTI-LANGUAGE AI AGENT RUNTIME
        </div>
      </div>
      <div style={{ opacity: subtitleOpacity, transform: `translateY(${taglineY}px)` }}>
        <div
          style={{
            fontSize: 32,
            color: "#e2e8f0",
            marginTop: 48,
            maxWidth: 800,
            textAlign: "center",
            lineHeight: 1.4,
          }}
        >
          One framework — Rust core, Python & JS bindings
        </div>
      </div>
    </AbsoluteFill>
  );
};

const FeatureSlide: React.FC = () => {
  const frame = useCurrentFrame();

  const features = [
    { icon: "🤖", title: "AI Agents", desc: "LLM integration with tools, skills & memory" },
    { icon: "🔌", title: "Event Bus", desc: "Pub/sub, queries, RPC for multi-agent comms" },
    { icon: "🛠️", title: "MCP Client", desc: "Native MCP server connectivity" },
    { icon: "⚡", title: "Production Ready", desc: "Circuit breakers, rate limiting, resilience" },
  ];

  return (
    <AbsoluteFill
      style={{
        backgroundColor: "#0f172a",
        justifyContent: "center",
        alignItems: "center",
        fontFamily: "Arial, sans-serif",
      }}
    >
      <div
        style={{
          fontSize: 48,
          fontWeight: 700,
          color: "#ffffff",
          marginBottom: 64,
        }}
      >
        Why BrainOS?
      </div>
      <div
        style={{
          display: "flex",
          gap: 32,
          flexWrap: "wrap",
          justifyContent: "center",
          maxWidth: 1200,
          padding: "0 48px",
        }}
      >
        {features.map((feature, index) => {
          const itemOpacity = interpolate(frame, [10 + index * 8, 30 + index * 8], [0, 1], {
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          });

          const itemY = interpolate(frame, [10 + index * 8, 30 + index * 8], [40, 0], {
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          });

          return (
            <div
              key={feature.title}
              style={{
                opacity: itemOpacity,
                transform: `translateY(${itemY}px)`,
                backgroundColor: "#1e293b",
                borderRadius: 16,
                padding: "32px 40px",
                width: 260,
                textAlign: "center",
                border: "1px solid #334155",
              }}
            >
              <div style={{ fontSize: 48, marginBottom: 16 }}>{feature.icon}</div>
              <div
                style={{
                  fontSize: 24,
                  fontWeight: 700,
                  color: "#ffffff",
                  marginBottom: 12,
                }}
              >
                {feature.title}
              </div>
              <div
                style={{
                  fontSize: 16,
                  color: "#94a3b8",
                  lineHeight: 1.4,
                }}
              >
                {feature.desc}
              </div>
            </div>
          );
        })}
      </div>
    </AbsoluteFill>
  );
};

const LanguagesSlide: React.FC = () => {
  const frame = useCurrentFrame();

  const languages = [
    { name: "Python", color: "#3776AB", code: 'from nbos import BrainOS\n\nasync with BrainOS() as brain:\n    agent = brain.agent("assistant")\n    result = await agent.ask("Hello!")' },
    { name: "JavaScript", color: "#CB3837", code: 'const { BrainOS } = require("brainos");\n\nconst brain = new BrainOS();\nawait brain.start();\nconst agent = brain.agent("assistant");' },
    { name: "Rust", color: "#DEA584", code: 'use agent::{Agent, AgentConfig};\n\nlet agent = Agent::builder()\n    .config(AgentConfig::default())\n    .build()?;' },
  ];

  return (
    <AbsoluteFill
      style={{
        backgroundColor: "#0f172a",
        justifyContent: "center",
        alignItems: "center",
        fontFamily: "Arial, sans-serif",
      }}
    >
      <div
        style={{
          fontSize: 48,
          fontWeight: 700,
          color: "#ffffff",
          marginBottom: 48,
        }}
      >
        Multi-Language Support
      </div>
      <div
        style={{
          display: "flex",
          gap: 24,
          flexWrap: "wrap",
          justifyContent: "center",
        }}
      >
        {languages.map((lang, index) => {
          const itemOpacity = interpolate(frame, [5 + index * 10, 25 + index * 10], [0, 1], {
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          });

          const itemScale = interpolate(frame, [5 + index * 10, 25 + index * 10], [0.9, 1], {
            extrapolateRight: "clamp",
            easing: Easing.bezier(0.16, 1, 0.3, 1),
          });

          return (
            <div
              key={lang.name}
              style={{
                opacity: itemOpacity,
                transform: `scale(${itemScale})`,
                backgroundColor: "#1e293b",
                borderRadius: 12,
                padding: 24,
                width: 340,
                border: `2px solid ${lang.color}`,
              }}
            >
              <div
                style={{
                  fontSize: 20,
                  fontWeight: 700,
                  color: lang.color,
                  marginBottom: 16,
                }}
              >
                {lang.name}
              </div>
              <pre
                style={{
                  fontSize: 12,
                  color: "#e2e8f0",
                  margin: 0,
                  fontFamily: "monospace",
                  lineHeight: 1.6,
                  overflow: "hidden",
                }}
              >
                {lang.code}
              </pre>
            </div>
          );
        })}
      </div>
    </AbsoluteFill>
  );
};

const QuickStartSlide: React.FC = () => {
  const frame = useCurrentFrame();

  const codeLines = [
    'pip install nbos',
    'python -c "',
    '  from nbos import BrainOS',
    '  import asyncio',
    '  asyncio.run(',
    '    BrainOS().agent("asst").ask("hi")',
    '  )',
    '"',
  ];

  return (
    <AbsoluteFill
      style={{
        backgroundColor: "#0f172a",
        justifyContent: "center",
        alignItems: "center",
        fontFamily: "Arial, sans-serif",
      }}
    >
      <div
        style={{
          fontSize: 48,
          fontWeight: 700,
          color: "#ffffff",
          marginBottom: 48,
        }}
      >
        30-Second Quick Start
      </div>
      <div
        style={{
          backgroundColor: "#000000",
          borderRadius: 12,
          padding: "32px 48px",
          border: "1px solid #334155",
        }}
      >
        <pre
          style={{
            fontSize: 20,
            color: "#22d3ee",
            margin: 0,
            fontFamily: "monospace",
            lineHeight: 1.8,
          }}
        >
          {codeLines.map((line, index) => {
            const lineOpacity = interpolate(frame, [5 + index * 4, 15 + index * 4], [0, 1], {
              extrapolateRight: "clamp",
            });
            return (
              <div key={index} style={{ opacity: lineOpacity }}>
                {line}
              </div>
            );
          })}
        </pre>
      </div>
      <div
        style={{
          marginTop: 48,
          fontSize: 24,
          color: "#94a3b8",
        }}
      >
        npm install @open1s/jsbos | cargo add agent
      </div>
    </AbsoluteFill>
  );
};

const EndSlide: React.FC = () => {
  const frame = useCurrentFrame();

  const opacity = interpolate(frame, [0, 20], [0, 1], {
    extrapolateRight: "clamp",
    easing: Easing.bezier(0.16, 1, 0.3, 1),
  });

  const scale = interpolate(frame, [0, 20], [0.9, 1], {
    extrapolateRight: "clamp",
    easing: Easing.bezier(0.16, 1, 0.3, 1),
  });

  return (
    <AbsoluteFill
      style={{
        backgroundColor: "#0f172a",
        justifyContent: "center",
        alignItems: "center",
        fontFamily: "Arial, sans-serif",
      }}
    >
      <div style={{ opacity, transform: `scale(${scale})`, textAlign: "center" }}>
        <div
          style={{
            fontSize: 80,
            fontWeight: 900,
            color: "#ffffff",
            marginBottom: 24,
          }}
        >
          BrainOS
        </div>
        <div
          style={{
            fontSize: 28,
            color: "#94a3b8",
            marginBottom: 48,
          }}
        >
          github.com/open1s/bos
        </div>
        <div
          style={{
            display: "flex",
            gap: 24,
            justifyContent: "center",
            marginTop: 32,
          }}
        >
          <Badge color="#3776AB" text="pip install nbos" />
          <Badge color="#CB3837" text="npm install @open1s/jsbos" />
          <Badge color="#DEA584" text="cargo add agent" />
        </div>
      </div>
    </AbsoluteFill>
  );
};

const Badge: React.FC<{ color: string; text: string }> = ({ color, text }) => {
  const frame = useCurrentFrame();

  const opacity = interpolate(frame, [40, 60], [0, 1], {
    extrapolateRight: "clamp",
  });

  const y = interpolate(frame, [40, 60], [20, 0], {
    extrapolateRight: "clamp",
    easing: Easing.bezier(0.16, 1, 0.3, 1),
  });

  return (
    <div
      style={{
        opacity,
        transform: `translateY(${y}px)`,
        backgroundColor: color,
        color: "#ffffff",
        padding: "12px 24px",
        borderRadius: 8,
        fontSize: 16,
        fontWeight: 600,
        fontFamily: "monospace",
      }}
    >
      {text}
    </div>
  );
};

export const MyComposition: React.FC = () => {
  return (
    <AbsoluteFill style={{ backgroundColor: "#0f172a" }}>
      <Sequence durationInFrames={90}>
        <TitleSlide />
      </Sequence>
      <Sequence from={95} durationInFrames={120}>
        <FeatureSlide />
      </Sequence>
      <Sequence from={220} durationInFrames={150}>
        <LanguagesSlide />
      </Sequence>
      <Sequence from={375} durationInFrames={120}>
        <QuickStartSlide />
      </Sequence>
      <Sequence from={500} durationInFrames={90}>
        <EndSlide />
      </Sequence>
    </AbsoluteFill>
  );
};