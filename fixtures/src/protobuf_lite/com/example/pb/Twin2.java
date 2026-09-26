package com.example.pb;

import com.google.protobuf.GeneratedMessageLite;

/** The shape protoc's lite codegen gives message Twin2. */
public final class Twin2 extends GeneratedMessageLite {
    public static final int A_FIELD_NUMBER = 1;
    public static final int B_FIELD_NUMBER = 2;
    public static final int C_FIELD_NUMBER = 3;
    private int a_;
    private int b_;
    private int c_;
    public Twin2(int a, int b, int c) {
        this.a_ = a;
        this.b_ = b;
        this.c_ = c;
    }
    @Override protected String describe() { return "T:" + a_ + "," + b_ + "," + c_; }
}
