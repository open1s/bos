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

import { BrainOS, Content, ContentPart, ConfigLoader, initTracing } from '../index.js';
import { readFileSync, existsSync } from 'fs';
import { resolve } from 'path';

const CAT_IMAGE_1 = 'https://download.catpng.net/silver_tabby_cat_on_gray_pillow_beside_clear_glass_window-thumbnail.png';
const CAT_IMAGE_2 = 'https://download.catpng.net/Three%20cats,%20including%20two%20kittens,%20perched%20on%20a%20stump%20in%20a%20picturesque%20garden%20setting._18887-thumbnail.png';
const AUDIO_FILE = "/Users/gaosg/Projects/bos/docs/audio.wav";
const AUDIO_FILE_PATH = "/Users/gaosg/Projects/bos/docs/audio.wav";

// initTracing();

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

async function demoTextOnly() {
    console.log('\n' + '='.repeat(60));
    console.log('Demo 1: Simple Text Content (Backward Compatible)');
    console.log('='.repeat(60));

    const modelConfig = await getModelConfig();
    console.log('Using model:', modelConfig.model);

    const brain = new BrainOS({
        model: modelConfig.model,
        baseUrl: modelConfig.baseUrl,
        apiKey: modelConfig.apiKey,
    });

    await brain.start();
    const agent = brain.agent('assistant');

    console.log('\n📤 Asking: What is Python?');
    const result = await agent.ask('What is Python?');
    console.log('📥 Agent:', result.slice(0, 200) + (result.length > 200 ? '...' : ''));

    await brain.stop();
}

async function demoContentText() {
    console.log('\n' + '='.repeat(60));
    console.log('Demo 2: Using Content.text()');
    console.log('='.repeat(60));

    const modelConfig = await getModelConfig();
    const brain = new BrainOS({
        model: modelConfig.model,
        baseUrl: modelConfig.baseUrl,
        apiKey: modelConfig.apiKey,
    });

    await brain.start();
    const agent = brain.agent('assistant');

    const content = Content.text('What is 2 + 2?');
    console.log('\n📤 Sending Content.text:', JSON.stringify(content.toJSON()));
    const result = await agent.ask(content);
    console.log('📥 Agent:', result.slice(0, 200) + (result.length > 200 ? '...' : ''));

    await brain.stop();
}

async function demoContentSingleImage() {
    console.log('\n' + '='.repeat(60));
    console.log('Demo 3: Single Image Content (Multimodal)');
    console.log('='.repeat(60));

    const modelConfig = await getModelConfig();
    const brain = new BrainOS({
        model: modelConfig.model,
        baseUrl: modelConfig.baseUrl,
        apiKey: modelConfig.apiKey,
    });

    await brain.start();
    const agent = brain.agent('assistant');

    console.log('\n📤 Sending cat image:', CAT_IMAGE_1);
    const content = Content.image(CAT_IMAGE_1);

    console.log('   Content JSON:', JSON.stringify(content.toJSON()).slice(0, 200) + '...');
    const result = await agent.ask(content);
    console.log('📥 Agent:', result.slice(0, 300) + (result.length > 300 ? '...' : ''));

    await brain.stop();
}

async function demoContentImageWithText() {
    console.log('\n' + '='.repeat(60));
    console.log('Demo 4: Text + Image Content (Multimodal)');
    console.log('='.repeat(60));

    const modelConfig = await getModelConfig();
    const brain = new BrainOS({
        model: modelConfig.model,
        baseUrl: modelConfig.baseUrl,
        apiKey: modelConfig.apiKey,
    });

    await brain.start();
    const agent = brain.agent('assistant');

    const content = Content.parts([
        ContentPart.text('What is in this image? Describe it in detail.'),
        ContentPart.image(CAT_IMAGE_1, 'high'),
    ]);

    console.log('\n📤 Sending text + image:');
    console.log('   Image URL:', CAT_IMAGE_1);
    console.log('   Content JSON:', JSON.stringify(content.toJSON(), null, 2).slice(0, 300) + '...');

    const result = await agent.ask(content);
    console.log('📥 Agent:', result.slice(0, 400) + (result.length > 400 ? '...' : ''));

    await brain.stop();
}

async function demoContentMultipleImages() {
    console.log('\n' + '='.repeat(60));
    console.log('Demo 5: Multiple Images Content (Multimodal)');
    console.log('='.repeat(60));

    const modelConfig = await getModelConfig();
    const brain = new BrainOS({
        model: modelConfig.model,
        baseUrl: modelConfig.baseUrl,
        apiKey: modelConfig.apiKey,
    });

    await brain.start();
    const agent = brain.agent('assistant');

    const content = Content.parts([
        ContentPart.text('I have two images for you. Describe both.'),
        ContentPart.image(CAT_IMAGE_1, 'high'),
        ContentPart.text('And here\'s a second image:'),
        ContentPart.image(CAT_IMAGE_2, 'high'),
    ]);

    console.log('\n📤 Sending text + 2 images:');
    console.log('   Image 1:', CAT_IMAGE_1);
    console.log('   Image 2:', CAT_IMAGE_2);

    const result = await agent.ask(content);
    console.log('📥 Agent:', result.slice(0, 500) + (result.length > 500 ? '...' : ''));

    await brain.stop();
}

async function demoContentCompareImages() {
    console.log('\n' + '='.repeat(60));
    console.log('Demo 6: Compare Two Images');
    console.log('='.repeat(60));

    const modelConfig = await getModelConfig();
    const brain = new BrainOS({
        model: modelConfig.model,
        baseUrl: modelConfig.baseUrl,
        apiKey: modelConfig.apiKey,
    });

    await brain.start();
    const agent = brain.agent('assistant');

    const content = Content.parts([
        ContentPart.text('Compare these two cat images. What are the similarities and differences?'),
        ContentPart.image(CAT_IMAGE_1),
        ContentPart.image(CAT_IMAGE_2),
    ]);

    console.log('\n📤 Comparing two cat images:');
    console.log('   Image 1:', CAT_IMAGE_1);
    console.log('   Image 2:', CAT_IMAGE_2);

    const result = await agent.ask(content);
    console.log('📥 Agent:', result.slice(0, 500) + (result.length > 500 ? '...' : ''));

    await brain.stop();
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

async function demoStream() {
    console.log('\n' + '='.repeat(60));
    console.log('Demo 8: Streaming Response');
    console.log('='.repeat(60));

    const modelConfig = await getModelConfig();
    const brain = new BrainOS({
        model: modelConfig.model,
        baseUrl: modelConfig.baseUrl,
        apiKey: modelConfig.apiKey,
    });

    await brain.start();
    const agent = brain.agent('assistant');

    const content = Content.text('Count from 1 to 5, one number per line.');

    console.log('\n📤 Asking: Count 1 to 5');
    const result = await agent.stream(content, (err, token) => {        
        if (token.type === 'ReasoningContent') {
            process.stdout.write(token.text);
        } else if (token.type === 'Text') {
            process.stdout.write(token.text);
        } else if (token.type === 'Usage') {
            console.log('\nToken:', token.totalTokens);
        }
    });
    console.log('');

    await brain.stop();
}

async function main() {
    console.log('='.repeat(60));
    console.log('  BrainOS Multimodal Demo (JavaScript)');
    console.log('  Config is loaded from ~/.bos/conf/config.toml');
    console.log('='.repeat(60));

try {
        // await demoTextOnly();
        // await demoContentText();
        // await demoContentSingleImage();
        // await demoContentImageWithText();
        // await demoContentMultipleImages();
        // await demoContentCompareImages();
        await demoContentAudio();
        // await demoStream();

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