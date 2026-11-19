// Clean-room bidirectional motion-compensated frame interpolation with occlusion & static-UI protection (cs_5_0).
cbuffer InterpParams : register(b0)
{
    uint2 FrameSize;
    float Alpha;       // interpolation factor in [0, 1]
    float UiThreshold; // static-UI confidence threshold (0 disables)
};

Texture2D<float4> PrevFrame : register(t0);
Texture2D<float4> CurrFrame : register(t1);
Texture2D<float2> MotionVectors : register(t2);
Texture2D<float> OcclusionMask : register(t3);
RWTexture2D<float4> OutputFrame : register(u0);
SamplerState LinearClamp : register(s0);

// CPU Vulkan trades boundary-repair searches for a bounded bilinear warp.
[numthreads(8, 8, 1)]
void cpu(uint3 tid : SV_DispatchThreadID)
{
    if (any(tid.xy >= FrameSize)) return;
    float4 p = PrevFrame.Load(int3(tid.xy, 0));
    float4 c = CurrFrame.Load(int3(tid.xy, 0));
    if (Alpha <= 0) { OutputFrame[tid.xy] = p; return; }
    if (Alpha >= 1) { OutputFrame[tid.xy] = c; return; }
    float4 direct = lerp(p, c, Alpha);
    float3 delta = abs(p.rgb - c.rgb);
    float2 uv = (float2(tid.xy) + 0.5f) / float2(FrameSize);
    float2 motionPixels = MotionVectors.SampleLevel(LinearClamp, uv, 0);
    if (max(delta.r, max(delta.g, delta.b)) < 0.002f && dot(motionPixels, motionPixels) < 0.25f)
    {
        OutputFrame[tid.xy] = direct;
        return;
    }
    float2 motion = motionPixels / float2(FrameSize);
    float confidence = saturate(OcclusionMask.SampleLevel(LinearClamp, uv, 0));
    float4 warped = lerp(
        PrevFrame.SampleLevel(LinearClamp, uv - motion * Alpha, 0),
        CurrFrame.SampleLevel(LinearClamp, uv + motion * (1.0f - Alpha), 0), Alpha);
    OutputFrame[tid.xy] = lerp(direct, warped, smoothstep(0.2f, 0.7f, confidence));
}

float2 AxialLumaGrad(Texture2D<float4> frame, float2 uv, float2 texel, float centerLuma, out float axialContrast)
{
    const float3 luma = float3(0.2126f, 0.7152f, 0.0722f);
    float rx = dot(frame.SampleLevel(LinearClamp, uv + float2(texel.x, 0.0f), 0).rgb, luma);
    float lx = dot(frame.SampleLevel(LinearClamp, uv - float2(texel.x, 0.0f), 0).rgb, luma);
    float dy = dot(frame.SampleLevel(LinearClamp, uv + float2(0.0f, texel.y), 0).rgb, luma);
    float uy = dot(frame.SampleLevel(LinearClamp, uv - float2(0.0f, texel.y), 0).rgb, luma);
    axialContrast = max(max(abs(rx - centerLuma), abs(lx - centerLuma)),
                        max(abs(dy - centerLuma), abs(uy - centerLuma)));
    return float2(rx - lx, dy - uy);
}

float SourceFlowError(float2 uv, float2 candidate, float2 invFrame, uint2 flowSize)
{
    float2 source = uv - Alpha * candidate * invFrame;
    int2 cell = clamp(int2(round(source * float2(flowSize) - 0.5f)), int2(0,0), int2(flowSize)-1);
    return length(MotionVectors.Load(int3(cell,0)) - candidate);
}

float2 WarpBarrier(float2 uv, float2 warp)
{
    const float3 luma = float3(0.2126f, 0.7152f, 0.0722f);
    float2 delta = warp - uv;
    float2 step = delta / 4.0f;
    float2 result = float2(0.0f, 1.0f);
    float p0 = dot(PrevFrame.SampleLevel(LinearClamp, uv, 0).rgb, luma);
    float c0 = dot(CurrFrame.SampleLevel(LinearClamp, uv, 0).rgb, luma);
    [unroll] for (int i = 1; i <= 4; ++i)
    {
        float2 at = uv + step * i;
        float p = dot(PrevFrame.SampleLevel(LinearClamp, at, 0).rgb, luma);
        float c = dot(CurrFrame.SampleLevel(LinearClamp, at, 0).rgb, luma);
        float g0 = p - p0;
        float g1 = c - c0;
        float strength = g0 * g1 > 0.0f ? min(abs(g0), abs(g1)) : 0.0f;
        if (strength > result.x)
            result = float2(strength, g0 > 0.0f ? saturate((p0 + c0) / max(0.01f, p + c)) : 1.0f);
        p0 = p;
        c0 = c;
    }
    return float2(smoothstep(0.08f, 0.19f, result.x), result.y);
}

float3 Chroma(float3 rgb)
{
    return rgb - (rgb.r + rgb.g + rgb.b) / 3.0f;
}

float StationaryChroma(float3 a, float3 b)
{
    float aa = dot(a, a);
    float bb = dot(b, b);
    float cosine = dot(a, b) * rsqrt(max(1e-5f, aa * bb));
    return smoothstep(0.005f, 0.015f, min(aa, bb)) * smoothstep(0.93f, 0.99f, cosine);
}

[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    if (tid.x >= FrameSize.x || tid.y >= FrameSize.y)
        return;

    if (Alpha <= 0.0f || Alpha >= 1.0f)
    {
        OutputFrame[tid.xy] = Alpha <= 0.0f ? PrevFrame.Load(int3(tid.xy, 0)) : CurrFrame.Load(int3(tid.xy, 0));
        return;
    }

    float2 uv = (float2(tid.xy) + 0.5f) / float2(FrameSize);
    uint2 flowSize;
    MotionVectors.GetDimensions(flowSize.x, flowSize.y);

    // Edge-preserving 4-tap bilateral MV + occlusion upsampling removes 8x8 block stepping without blurring motion edges.
    float2 fc = uv * float2(flowSize) - 0.5f;
    int2 i0 = int2(floor(fc));
    float2 f = fc - float2(i0);
    int2 maxFlow = int2(flowSize) - 1;

    int2 p00 = clamp(i0, int2(0, 0), maxFlow);
    int2 p10 = clamp(i0 + int2(1, 0), int2(0, 0), maxFlow);
    int2 p01 = clamp(i0 + int2(0, 1), int2(0, 0), maxFlow);
    int2 p11 = clamp(i0 + int2(1, 1), int2(0, 0), maxFlow);

    int2 nearestFlow = clamp(int2(round(fc)), int2(0, 0), maxFlow);
    float2 mvNearest = MotionVectors.Load(int3(nearestFlow, 0));
    float occNearest = saturate(OcclusionMask.Load(int3(nearestFlow, 0)));

    float2 mv00 = MotionVectors.Load(int3(p00, 0));
    float2 mv10 = MotionVectors.Load(int3(p10, 0));
    float2 mv01 = MotionVectors.Load(int3(p01, 0));
    float2 mv11 = MotionVectors.Load(int3(p11, 0));

    float occ00 = saturate(OcclusionMask.Load(int3(p00, 0)));
    float occ10 = saturate(OcclusionMask.Load(int3(p10, 0)));
    float occ01 = saturate(OcclusionMask.Load(int3(p01, 0)));
    float occ11 = saturate(OcclusionMask.Load(int3(p11, 0)));

    float2 d00 = mv00 - mvNearest;
    float2 d10 = mv10 - mvNearest;
    float2 d01 = mv01 - mvNearest;
    float2 d11 = mv11 - mvNearest;

    float w00 = (1.0f - f.x) * (1.0f - f.y) * saturate(1.0f - dot(d00, d00) / 4.0f) * lerp(0.25f, 1.0f, occ00);
    float w10 = f.x * (1.0f - f.y) * saturate(1.0f - dot(d10, d10) / 4.0f) * lerp(0.25f, 1.0f, occ10);
    float w01 = (1.0f - f.x) * f.y * saturate(1.0f - dot(d01, d01) / 4.0f) * lerp(0.25f, 1.0f, occ01);
    float w11 = f.x * f.y * saturate(1.0f - dot(d11, d11) / 4.0f) * lerp(0.25f, 1.0f, occ11);

    float wSum = w00 + w10 + w01 + w11;
    float2 mvPx = (wSum > 1e-4f) ? (mv00 * w00 + mv10 * w10 + mv01 * w01 + mv11 * w11) / wSum : mvNearest;
    float occConf = (wSum > 1e-4f) ? (occ00 * w00 + occ10 * w10 + occ01 * w01 + occ11 * w11) / wSum : occNearest;
    float2 invFrame = 1.0f / float2(FrameSize);
    float2 mv = mvPx * invFrame;

    float2 uvPrev = saturate(uv - mv * Alpha);
    float2 uvCurr = saturate(uv + mv * (1.0f - Alpha));

    float4 prevSample = PrevFrame.SampleLevel(LinearClamp, uvPrev, 0);
    float4 currSample = CurrFrame.SampleLevel(LinearClamp, uvCurr, 0);
    float diff = length(prevSample.rgb - currSample.rgb);

    // 1-step forward advection check along Prev(p)->Curr(p+mv) at motion boundaries.
    float2 uvAdv = saturate(uv - Alpha * mv);
    int2 advFlow = clamp(int2(round(uvAdv * float2(flowSize) - 0.5f)), int2(0, 0), maxFlow);
    float2 mvAdv = MotionVectors.Load(int3(advFlow, 0));
    float2 advDelta = mvAdv - mvPx;
    float supportError = length(advDelta);
    [branch]
    if (dot(advDelta, advDelta) > 1.0f)
    {
        float2 advUvPrev = saturate(uv - (mvAdv * invFrame) * Alpha);
        float2 advUvCurr = saturate(uv + (mvAdv * invFrame) * (1.0f - Alpha));
        float4 advPrev = PrevFrame.SampleLevel(LinearClamp, advUvPrev, 0);
        float4 advCurr = CurrFrame.SampleLevel(LinearClamp, advUvCurr, 0);
        float advDiff = length(advPrev.rgb - advCurr.rgb);
        float advSupport = SourceFlowError(uv,mvAdv,invFrame,flowSize);
        if (advDiff + min(advSupport,4.0f)*0.125f < diff + min(supportError,4.0f)*0.125f - 0.01f)
        {
            supportError = advSupport;
            mvPx = mvAdv;
            mv = mvPx * invFrame;
            uvPrev = advUvPrev;
            uvCurr = advUvCurr;
            prevSample = advPrev;
            currSample = advCurr;
            diff = advDiff;
            occConf = max(occConf, saturate(OcclusionMask.Load(int3(advFlow, 0))));
        }
    }

    // Compare each cross-cell candidate at its advected source coordinates, including zero motion.
    [branch] if (occConf < 0.40f || (diff > 0.15f && occConf < 0.65f) || supportError > 3.0f)
    {
        const int2 repairDirs[4] = { int2(-2,0), int2(2,0), int2(0,-2), int2(0,2) };
        [unroll] for (int ri = 0; ri < 4; ++ri)
        {
            int2 nc = clamp(nearestFlow + repairDirs[ri], int2(0,0), maxFlow);
            float2 candidate = MotionVectors.Load(int3(nc,0));
            if (all(candidate == mvPx)) continue;
            float2 p = uv - candidate * invFrame * Alpha;
            float2 q = uv + candidate * invFrame * (1.0f-Alpha);
            if (any(p < 0) || any(p > 1) || any(q < 0) || any(q > 1)) continue;
            float4 ps = PrevFrame.SampleLevel(LinearClamp,p,0);
            float4 qs = CurrFrame.SampleLevel(LinearClamp,q,0);
            float error = length(ps.rgb-qs.rgb);
            float candidateSupport = SourceFlowError(uv,candidate,invFrame,flowSize);
            if (error + min(candidateSupport,4.0f)*0.125f < diff + min(supportError,4.0f)*0.125f - 0.005f)
            {
                supportError = candidateSupport;
                mvPx = candidate; mv = candidate * invFrame;
                uvPrev = p; uvCurr = q; prevSample = ps; currSample = qs; diff = error;
                occConf = saturate(OcclusionMask.Load(int3(nc,0)));
            }
        }
    }

    float4 linearPrev = PrevFrame.Load(int3(tid.xy, 0));
    float4 linearCurr = CurrFrame.Load(int3(tid.xy, 0));

    // Protect opaque and semi-transparent/anti-aliased HUDs via axial luma gradient agreement.
    float staticDiff = length(linearPrev.rgb - linearCurr.rgb);
    float uiGate = min(UiThreshold * 0.45f, 0.22f);
    float staticMask = 0.0f;
    [branch]
    if (UiThreshold > 0.0f && staticDiff < uiGate)
    {
        const float3 luma = float3(0.2126f, 0.7152f, 0.0722f);
        float cPrev = 0.0f;
        float cCurr = 0.0f;
        float2 gPrev = AxialLumaGrad(PrevFrame, uv, invFrame, dot(linearPrev.rgb, luma), cPrev);
        float2 gCurr = AxialLumaGrad(CurrFrame, uv, invFrame, dot(linearCurr.rgb, luma), cCurr);
        float edge = min(length(gPrev), length(gCurr));
        float gradErr = length(gPrev - gCurr) / (length(gPrev) + length(gCurr) + 0.02f);

        float diffThresh = min(UiThreshold, 0.025f);
        float contrast = max(cPrev, cCurr);
        float staticSteady = (staticDiff < 0.004f) ? 1.0f : saturate(1.0f - staticDiff / max(contrast * 0.35f, 0.005f));
        // Opposing edges cancel the centered gradient of thin opaque lines.
        float lineContrast = min(cPrev, cCurr);
        float thinLine = smoothstep(0.08f, 0.16f, lineContrast);
        float opaqueMask = (1.0f - saturate(staticDiff / max(diffThresh, 1e-4f))) * staticSteady
            * max(thinLine, 1.0f - smoothstep(0.02f, 0.10f, gradErr));

        float transUpper = max(0.09f, uiGate);
        float transLo = min(0.08f, transUpper - 0.01f);
        float transHud = smoothstep(0.03f, 0.10f, edge)
            * (1.0f - smoothstep(0.28f, 0.62f, gradErr))
            * (1.0f - smoothstep(transLo, transUpper, staticDiff))
            * smoothstep(0.01f, 0.06f, diff - staticDiff);

        staticMask = max(opaqueMask, transHud);
    }

    // Visibility is explicit: clamping a coordinate does not make an unavailable source valid.
    float2 sourcePrev = uv - mv * Alpha;
    float2 sourceCurr = uv + mv * (1.0f - Alpha);
    float prevValid = all(sourcePrev >= 0.5f * invFrame) && all(sourcePrev <= 1.0f - 0.5f * invFrame) ? 1.0f : 0.0f;
    float currValid = all(sourceCurr >= 0.5f * invFrame) && all(sourceCurr <= 1.0f - 0.5f * invFrame) ? 1.0f : 0.0f;
    float archWeight = 4.0f * Alpha * (1.0f - Alpha);
    // A disoccluding edge can disagree in both warps. Compare against the error
    // of the unwarped samples before blending back to them.
    float chromaMismatch = length(Chroma(prevSample.rgb - currSample.rgb));
    float warpRelief = 0.75f * staticDiff * (1.0f - smoothstep(0.03f, 0.12f, chromaMismatch));
    float warpResidual = max(0.0f, diff - warpRelief);
    float badPhoto = smoothstep(0.06f, 0.22f, warpResidual);
    float badFlow = (1.0f - occConf) * smoothstep(0.045f, 0.16f, warpResidual);
    float bad = max(badPhoto, badFlow);
    float4 unwarped = lerp(linearPrev, linearCurr, Alpha);
    float4 hudTarget = lerp(linearCurr, unwarped, 1.0f - archWeight);
    float4 nearestReal = lerp(linearPrev, linearCurr, smoothstep(0.0f, 1.0f, Alpha));
    float visibilityWeight = prevValid * (1.0f - Alpha) + currValid * Alpha;
    float4 baseWarped = visibilityWeight > 0.0f
        ? (prevSample * prevValid * (1.0f - Alpha) + currSample * currValid * Alpha) / visibilityWeight
        : nearestReal;
    float4 warped = lerp(baseWarped, nearestReal, bad * archWeight);

    // Prefer the warp that stays on the same side of a screen-stationary edge.
    [branch] if (UiThreshold > 0.0f && diff > 0.15f && occConf > 0.4f && dot(mvPx, mvPx) > 64.0f
        && staticDiff < diff * 0.8f && prevValid * currValid > 0.0f)
    {
        float2 prevBarrier = WarpBarrier(uv, uvPrev);
        float2 currBarrier = WarpBarrier(uv, uvCurr);
        float edge = prevBarrier.x - currBarrier.x;
        warped = lerp(warped, edge > 0.0f ? currSample : prevSample, abs(edge));
        float paired = min(prevBarrier.x, currBarrier.x)
            * (1.0f - smoothstep(0.15f, 0.40f, abs(prevBarrier.y - currBarrier.y)));
        float3 attenuated = lerp(prevSample.rgb * prevBarrier.y, currSample.rgb * currBarrier.y, Alpha);
        warped.rgb = lerp(warped.rgb, attenuated, paired);
    }


    // Resolve uncertain visibility without snapping between screen positions.
    float uncovered = smoothstep(2.0f, 3.0f, supportError);
    [branch] if (uncovered > 0.0f)
    {
        int2 cell = clamp(int2(round(uvPrev * float2(flowSize) - 0.5f)), int2(0,0), maxFlow);
        float2 sourceMotion = MotionVectors.Load(int3(cell, 0));
        float useWarp = currValid * smoothstep(0.25f, 1.0f, dot(sourceMotion, sourceMotion));
        // Do not advect a stationary colored feature through a disocclusion.
        if (UiThreshold > 0.0f && useWarp > 0.0f)
        {
            float3 fixedCurrent = Chroma(linearCurr.rgb);
            float3 movingCurrent = Chroma(currSample.rgb);
            float center = StationaryChroma(fixedCurrent, Chroma(linearPrev.rgb));
            float passed = StationaryChroma(movingCurrent, Chroma(PrevFrame.SampleLevel(LinearClamp, uvCurr, 0).rgb));
            float chromaMove = smoothstep(0.05f, 0.16f, length(fixedCurrent - movingCurrent));
            useWarp *= 1.0f - chromaMove * max(center, passed);
        }
        warped = lerp(warped, lerp(linearCurr, currSample, useWarp), uncovered);
    }

    // Gated 4-tap clamped high-frequency restore on confident moving non-HUD pixels with fractional sampling blur.
    [branch]
    if (uncovered < 0.05f && prevValid * currValid > 0.0f && staticMask < 0.25f && occConf > 0.60f && bad < 0.25f && archWeight > 0.10f && dot(mvPx, mvPx) > 0.25f)
    {
        float2 fp = frac(uvPrev * float2(FrameSize) - 0.5f);
        float2 fc2 = frac(uvCurr * float2(FrameSize) - 0.5f);
        float2 bp = 2.0f * min(fp, 1.0f - fp);
        float2 bc = 2.0f * min(fc2, 1.0f - fc2);
        float subBlur = max(max(bp.x, bp.y), max(bc.x, bc.y));
        [branch]
        if (subBlur > 0.28f)
        {
            float3 cR = CurrFrame.SampleLevel(LinearClamp, saturate(uvCurr + float2(invFrame.x, 0.0f)), 0).rgb;
            float3 cL = CurrFrame.SampleLevel(LinearClamp, saturate(uvCurr - float2(invFrame.x, 0.0f)), 0).rgb;
            float3 cD = CurrFrame.SampleLevel(LinearClamp, saturate(uvCurr + float2(0.0f, invFrame.y)), 0).rgb;
            float3 cU = CurrFrame.SampleLevel(LinearClamp, saturate(uvCurr - float2(0.0f, invFrame.y)), 0).rgb;
            float3 nMin = min(min(cR, cL), min(cD, cU));
            float3 nMax = max(max(cR, cL), max(cD, cU));
            float3 detail = currSample.rgb - 0.25f * (cR + cL + cD + cU);
            float crispGain = 0.22f * subBlur * archWeight * (1.0f - bad)
                * smoothstep(0.60f, 0.85f, occConf)
                * (1.0f - smoothstep(0.0f, 0.25f, staticMask));
            warped.rgb = clamp(warped.rgb + detail * crispGain, min(warped.rgb, nMin), max(warped.rgb, nMax));
        }
    }

    if (UiThreshold > 0.0f)
    {
        float3 prevChroma = Chroma(linearPrev.rgb);
        float3 currChroma = Chroma(linearCurr.rgb);
        float3 warpedChroma = Chroma(warped.rgb);
        float changedHue = smoothstep(0.045f, 0.15f, length(warpedChroma - currChroma));
        float colorAnchor = StationaryChroma(prevChroma, currChroma) * changedHue;
        staticMask = max(staticMask, colorAnchor);
    }

    OutputFrame[tid.xy] = lerp(warped, hudTarget, staticMask);
}
