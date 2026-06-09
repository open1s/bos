/**
/**
 * BrainOS Multimodal Demo (JavaScript/Node.js)
 *
 * Shows how to use Content, ContentPart, ContentPart.text/image/audio
 * to send multimodal messages (text + images + audio) to LLM.
 *
 * Configuration is loaded from config.toml or environment variables.
 * Do NOT hardcode API keys - use config instead.
 *
 * Config file (~/.bos/conf/config.toml):
 *   [llm.google]
 *   model = "nvidia/google/gemma-4-31b-it"
 *   base_url = "http://127.0.0.1:11436/v1"
 *   api_key = "your-api-key-here"
 *
 * Usage:
 *   node demo_multimodal.js
 */

import { BrainOS, Content, ContentPart, ConfigLoader,initTracing } from '../index.js';
import { readFileSync, existsSync } from 'fs';
import { resolve } from 'path';

// initTracing();

const AUDIO_FILE = "/Users/gaosg/Projects/bos/docs/audio.wav";

async function getModelConfig() {
    const loader = new ConfigLoader();
    loader.discover();
    const config = JSON.parse(loader.loadSync());

    const googleConfig = config?.llm?.google || {};
    const nvidiaConfig = config?.llm?.nvidia || {};
    const globalModel = config?.global_model || {};

    const modelConfig = Object.keys(googleConfig).length ? googleConfig :
                        Object.keys(nvidiaConfig).length ? nvidiaConfig : globalModel;

    return {
        model: modelConfig.model || 'nvidia/google/gemma-4-31b-it',
        baseUrl: modelConfig.base_url || 'http://127.0.0.1:11436/v1',
        apiKey: modelConfig.api_key || process.env.NVIDIA_API_KEY || '',
    };
}

async function demoContentAudio() {
    console.log('\n' + '='.repeat(60));
    console.log('Demo 7: Text + Audio Content (Multimodal)');
    console.log('='.repeat(60));

    const modelConfig = await getModelConfig();
    const brain = new BrainOS({
        model: modelConfig.model,
        baseUrl: modelConfig.baseUrl,
        apiKey: modelConfig.apiKey,
    });

    await brain.start();
    const agent = brain.agent('assistant');

    const audioPath = process.env.AUDIO_FILE_PATH || AUDIO_FILE;

    let content;
    let audioData;

    if (existsSync(audioPath)) {
        audioData = readFileSync(audioPath).toString('base64');
        console.log('\n📤 Sending audio from file:', audioPath);
    } else {
        console.log('\n📤 Audio file not found:', audioPath);
        console.log('   Set AUDIO_FILE_PATH env var to use a different audio file');
        await brain.stop();
        return;
    }

    content = Content.parts([
        ContentPart.text('What does this audio say? Summarize briefly.'),
        ContentPart.audio(audioData, 'wav'),
    ]);

    console.log('   Audio data length:', audioData.length, 'bytes');
    console.log('   Content JSON:', JSON.stringify(content.toJSON()).slice(0, 150) + '...');

    try {
        const result = await agent.ask(content);
        console.log('📥 Agent:', result.slice(0, 300) + (result.length > 300 ? '...' : ''));
    } catch (err) {
        console.log('⚠️  Audio request failed:', err.message.slice(0, 150));
    }

    await brain.stop();
}

async function main() {
    console.log('='.repeat(60));
    console.log('  BrainOS Multimodal Demo (JavaScript)');
    console.log('  Config is loaded from ~/.bos/conf/config.toml');
    console.log('='.repeat(60));

    try {
        await demoContentAudio();

        console.log('\n' + '='.repeat(60));
        console.log('✅ All demos completed!');
        console.log('='.repeat(60));
    } catch (error) {
        console.error('❌ Demo failed:', error.message);
        if (error.message.includes('502') || error.message.includes('429')) {
            console.log('\n⚠️  The LLM API returned an error.');
            console.log('   Make sure the local Gemma endpoint is running');
            console.log('   and your config.toml is set up correctly.');
            console.log('\n   Config file (~/.bos/conf/config.toml) should have:');
            console.log('   [llm.google]');
            console.log('   model = "nvidia/google/gemma-4-31b-it"');
            console.log('   base_url = "http://127.0.0.1:11436/v1"');
            console.log('   api_key = "your-api-key"');
        }
    }
}

main();